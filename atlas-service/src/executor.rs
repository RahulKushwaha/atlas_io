use std::os::fd::RawFd;

/// Result of an I/O operation: bytes transferred or negative errno.
pub type IoResult = i64;

/// Trait abstracting the low-level I/O backend.
/// Implementations: PosixExecutor (pread/pwrite), IoUringExecutor (Linux).
pub trait IoExecutor {
    fn open(&mut self, path: &str, flags: i32, mode: u32) -> IoResult;
    fn close(&mut self, fd: RawFd) -> IoResult;
    fn pread(&mut self, fd: RawFd, buf: *mut u8, len: usize, offset: u64) -> IoResult;
    fn pwrite(&mut self, fd: RawFd, buf: *const u8, len: usize, offset: u64) -> IoResult;
    fn fsync(&mut self, fd: RawFd) -> IoResult;
}

/// Synchronous POSIX I/O — works on all platforms.
pub struct PosixExecutor;

impl PosixExecutor {
    pub fn new() -> Self { Self }
}

impl IoExecutor for PosixExecutor {
    fn open(&mut self, path: &str, flags: i32, mode: u32) -> IoResult {
        let c_path = match std::ffi::CString::new(path) {
            Ok(p) => p,
            Err(_) => return -(libc::EINVAL as i64),
        };
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
        if fd < 0 { -(errno() as i64) } else { fd as i64 }
    }

    fn close(&mut self, fd: RawFd) -> IoResult {
        let rc = unsafe { libc::close(fd) };
        if rc < 0 { -(errno() as i64) } else { 0 }
    }

    fn pread(&mut self, fd: RawFd, buf: *mut u8, len: usize, offset: u64) -> IoResult {
        let n = unsafe { libc::pread(fd, buf as *mut libc::c_void, len, offset as i64) };
        if n < 0 { -(errno() as i64) } else { n as i64 }
    }

    fn pwrite(&mut self, fd: RawFd, buf: *const u8, len: usize, offset: u64) -> IoResult {
        let n = unsafe { libc::pwrite(fd, buf as *const libc::c_void, len, offset as i64) };
        if n < 0 { -(errno() as i64) } else { n as i64 }
    }

    fn fsync(&mut self, fd: RawFd) -> IoResult {
        let rc = unsafe { libc::fsync(fd) };
        if rc < 0 { -(errno() as i64) } else { 0 }
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// io_uring-based executor — Linux only.
/// Submits each op as a single SQE and waits for completion.
/// For batched submission, the service can call multiple ops then flush.
#[cfg(target_os = "linux")]
pub mod uring {
    use super::{IoExecutor, IoResult};
    use io_uring::{opcode, types, IoUring};
    use std::os::fd::RawFd;

    pub struct IoUringExecutor {
        ring: IoUring,
    }

    impl IoUringExecutor {
        pub fn new(entries: u32) -> std::io::Result<Self> {
            Ok(Self { ring: IoUring::new(entries)? })
        }

        fn submit_and_wait_one(&mut self) -> IoResult {
            self.ring.submit_and_wait(1).ok();
            match self.ring.completion().next() {
                Some(cqe) => {
                    let ret = cqe.result();
                    if ret < 0 { ret as i64 } else { ret as i64 }
                }
                None => -(libc::EIO as i64),
            }
        }
    }

    impl IoExecutor for IoUringExecutor {
        fn open(&mut self, path: &str, flags: i32, mode: u32) -> IoResult {
            // open is not well-suited for io_uring single-shot; use syscall directly
            let c_path = match std::ffi::CString::new(path) {
                Ok(p) => p,
                Err(_) => return -(libc::EINVAL as i64),
            };
            let entry = opcode::OpenAt::new(types::Fd(libc::AT_FDCWD), c_path.as_ptr())
                .flags(flags)
                .mode(mode)
                .build()
                .user_data(0);
            unsafe { self.ring.submission().push(&entry).ok(); }
            self.submit_and_wait_one()
        }

        fn close(&mut self, fd: RawFd) -> IoResult {
            let entry = opcode::Close::new(types::Fd(fd))
                .build()
                .user_data(0);
            unsafe { self.ring.submission().push(&entry).ok(); }
            self.submit_and_wait_one()
        }

        fn pread(&mut self, fd: RawFd, buf: *mut u8, len: usize, offset: u64) -> IoResult {
            let entry = opcode::Read::new(types::Fd(fd), buf, len as u32)
                .offset(offset)
                .build()
                .user_data(0);
            unsafe { self.ring.submission().push(&entry).ok(); }
            self.submit_and_wait_one()
        }

        fn pwrite(&mut self, fd: RawFd, buf: *const u8, len: usize, offset: u64) -> IoResult {
            let entry = opcode::Write::new(types::Fd(fd), buf, len as u32)
                .offset(offset)
                .build()
                .user_data(0);
            unsafe { self.ring.submission().push(&entry).ok(); }
            self.submit_and_wait_one()
        }

        fn fsync(&mut self, fd: RawFd) -> IoResult {
            let entry = opcode::Fsync::new(types::Fd(fd))
                .build()
                .user_data(0);
            unsafe { self.ring.submission().push(&entry).ok(); }
            self.submit_and_wait_one()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tmp_path() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.dat");
        (dir, p.to_str().unwrap().to_string())
    }

    #[test]
    fn open_create_and_close() {
        let mut ex = PosixExecutor::new();
        let (_dir, path) = tmp_path();
        let fd = ex.open(&path, libc::O_CREAT | libc::O_RDWR, 0o644);
        assert!(fd > 0, "open returned {fd}");
        assert_eq!(ex.close(fd as RawFd), 0);
    }

    #[test]
    fn open_nonexistent_fails() {
        let mut ex = PosixExecutor::new();
        let rc = ex.open("/tmp/atlas_no_such_file_ever_12345", libc::O_RDONLY, 0);
        assert!(rc < 0);
    }

    #[test]
    fn write_read_roundtrip() {
        let mut ex = PosixExecutor::new();
        let (_dir, path) = tmp_path();

        let fd = ex.open(&path, libc::O_CREAT | libc::O_RDWR, 0o644);
        assert!(fd > 0);
        let fd = fd as RawFd;

        let data = b"hello posix executor";
        let written = ex.pwrite(fd, data.as_ptr(), data.len(), 0);
        assert_eq!(written, data.len() as i64);

        let mut buf = vec![0u8; 64];
        let n = ex.pread(fd, buf.as_mut_ptr(), buf.len(), 0);
        assert_eq!(n, data.len() as i64);
        assert_eq!(&buf[..n as usize], data);

        ex.close(fd);
    }

    #[test]
    fn pwrite_at_offset() {
        let mut ex = PosixExecutor::new();
        let (_dir, path) = tmp_path();

        let fd = ex.open(&path, libc::O_CREAT | libc::O_RDWR, 0o644) as RawFd;

        let a = b"AAAA";
        let b = b"BB";
        ex.pwrite(fd, a.as_ptr(), a.len(), 0);
        ex.pwrite(fd, b.as_ptr(), b.len(), 4);

        let mut buf = [0u8; 6];
        let n = ex.pread(fd, buf.as_mut_ptr(), 6, 0);
        assert_eq!(n, 6);
        assert_eq!(&buf, b"AAAABB");

        ex.close(fd);
    }

    #[test]
    fn fsync_succeeds() {
        let mut ex = PosixExecutor::new();
        let (_dir, path) = tmp_path();

        let fd = ex.open(&path, libc::O_CREAT | libc::O_RDWR, 0o644) as RawFd;
        ex.pwrite(fd, b"data".as_ptr(), 4, 0);
        assert_eq!(ex.fsync(fd), 0);
        ex.close(fd);
    }

    #[test]
    fn fsync_bad_fd() {
        let mut ex = PosixExecutor::new();
        assert!(ex.fsync(99999) < 0);
    }

    #[test]
    fn pread_bad_fd() {
        let mut ex = PosixExecutor::new();
        let mut buf = [0u8; 4];
        assert!(ex.pread(99999, buf.as_mut_ptr(), 4, 0) < 0);
    }

    #[test]
    fn close_bad_fd() {
        let mut ex = PosixExecutor::new();
        assert!(ex.close(99999) < 0);
    }

    #[test]
    fn fsync_persists_data() {
        let mut ex = PosixExecutor::new();
        let (_dir, path) = tmp_path();

        let fd = ex.open(&path, libc::O_CREAT | libc::O_RDWR, 0o644) as RawFd;
        let data = b"persistent";
        ex.pwrite(fd, data.as_ptr(), data.len(), 0);
        ex.fsync(fd);
        ex.close(fd);

        // Verify via std::fs
        let mut contents = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut contents).unwrap();
        assert_eq!(contents, data);
    }
}
