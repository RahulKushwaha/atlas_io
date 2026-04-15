use std::os::fd::RawFd;

/// Result of an I/O operation: bytes transferred or negative errno.
pub type IoResult = i64;

/// A single batchable data-plane op. Structural ops (open, close) are
/// not included here; they always run through the single-op methods
/// because they mutate `fd_table` state that downstream ops depend on.
pub enum BatchKind {
    Pread {
        fd: RawFd,
        buf: *mut u8,
        len: usize,
        offset: u64,
    },
    Pwrite {
        fd: RawFd,
        buf: *const u8,
        len: usize,
        offset: u64,
    },
    Fsync {
        fd: RawFd,
    },
}

/// An op paired with its result slot, filled in by `execute_batch`.
pub struct BatchOp {
    pub kind: BatchKind,
    pub result: IoResult,
}

impl BatchOp {
    pub fn new(kind: BatchKind) -> Self {
        Self { kind, result: 0 }
    }
}

/// Trait abstracting the low-level I/O backend.
/// Implementations: PosixExecutor (pread/pwrite), IoUringExecutor (Linux).
pub trait IoExecutor {
    fn open(&mut self, path: &str, flags: i32, mode: u32) -> IoResult;
    fn close(&mut self, fd: RawFd) -> IoResult;
    fn pread(&mut self, fd: RawFd, buf: *mut u8, len: usize, offset: u64) -> IoResult;
    fn pwrite(&mut self, fd: RawFd, buf: *const u8, len: usize, offset: u64) -> IoResult;
    fn fsync(&mut self, fd: RawFd) -> IoResult;

    /// Execute a batch of data-plane ops. The default runs them sequentially
    /// through the single-op methods; backends that support batching (io_uring)
    /// override this to submit all ops in one syscall and reap them together,
    /// amortising the kernel transition cost across the batch.
    fn execute_batch(&mut self, ops: &mut [BatchOp]) {
        for op in ops.iter_mut() {
            op.result = match op.kind {
                BatchKind::Pread {
                    fd,
                    buf,
                    len,
                    offset,
                } => self.pread(fd, buf, len, offset),
                BatchKind::Pwrite {
                    fd,
                    buf,
                    len,
                    offset,
                } => self.pwrite(fd, buf, len, offset),
                BatchKind::Fsync { fd } => self.fsync(fd),
            };
        }
    }
}

/// Synchronous POSIX I/O — works on all platforms.
pub struct PosixExecutor;

impl PosixExecutor {
    pub fn new() -> Self {
        Self
    }
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
    use super::{BatchKind, BatchOp, IoExecutor, IoResult};
    use io_uring::{IoUring, opcode, types};
    use std::os::fd::RawFd;

    pub struct IoUringExecutor {
        ring: IoUring,
        capacity: u32,
    }

    impl IoUringExecutor {
        pub fn new(entries: u32) -> std::io::Result<Self> {
            Ok(Self {
                ring: IoUring::new(entries)?,
                capacity: entries,
            })
        }

        fn submit_and_wait_one(&mut self) -> IoResult {
            self.ring.submit_and_wait(1).ok();
            match self.ring.completion().next() {
                Some(cqe) => cqe.result() as i64,
                None => -(libc::EIO as i64),
            }
        }

        fn build_entry(kind: &BatchKind, tag: u64) -> io_uring::squeue::Entry {
            match *kind {
                BatchKind::Pread {
                    fd,
                    buf,
                    len,
                    offset,
                } => opcode::Read::new(types::Fd(fd), buf, len as u32)
                    .offset(offset)
                    .build()
                    .user_data(tag),
                BatchKind::Pwrite {
                    fd,
                    buf,
                    len,
                    offset,
                } => opcode::Write::new(types::Fd(fd), buf, len as u32)
                    .offset(offset)
                    .build()
                    .user_data(tag),
                BatchKind::Fsync { fd } => opcode::Fsync::new(types::Fd(fd)).build().user_data(tag),
            }
        }

        fn submit_chunk(&mut self, ops: &mut [BatchOp], base: usize) {
            unsafe {
                let mut sq = self.ring.submission();
                for (i, op) in ops.iter().enumerate() {
                    let entry = Self::build_entry(&op.kind, (base + i) as u64);
                    // ring was sized to hold at least `ops.len()` entries (chunking guarantees this).
                    sq.push(&entry).expect("io_uring SQ push (sized)");
                }
            }
            self.ring.submit_and_wait(ops.len()).ok();
            // Reap completions for this chunk. user_data encodes the
            // global index into the caller's slice; we translate back
            // via `base` in the caller.
            for cqe in self.ring.completion() {
                let idx = cqe.user_data() as usize;
                if idx >= base && idx < base + ops.len() {
                    ops[idx - base].result = cqe.result() as i64;
                }
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
            unsafe {
                self.ring.submission().push(&entry).ok();
            }
            self.submit_and_wait_one()
        }

        fn close(&mut self, fd: RawFd) -> IoResult {
            let entry = opcode::Close::new(types::Fd(fd)).build().user_data(0);
            unsafe {
                self.ring.submission().push(&entry).ok();
            }
            self.submit_and_wait_one()
        }

        fn pread(&mut self, fd: RawFd, buf: *mut u8, len: usize, offset: u64) -> IoResult {
            let entry = opcode::Read::new(types::Fd(fd), buf, len as u32)
                .offset(offset)
                .build()
                .user_data(0);
            unsafe {
                self.ring.submission().push(&entry).ok();
            }
            self.submit_and_wait_one()
        }

        fn pwrite(&mut self, fd: RawFd, buf: *const u8, len: usize, offset: u64) -> IoResult {
            let entry = opcode::Write::new(types::Fd(fd), buf, len as u32)
                .offset(offset)
                .build()
                .user_data(0);
            unsafe {
                self.ring.submission().push(&entry).ok();
            }
            self.submit_and_wait_one()
        }

        fn fsync(&mut self, fd: RawFd) -> IoResult {
            let entry = opcode::Fsync::new(types::Fd(fd)).build().user_data(0);
            unsafe {
                self.ring.submission().push(&entry).ok();
            }
            self.submit_and_wait_one()
        }

        /// Batch submission: push all SQEs, one `submit_and_wait`, then
        /// reap all CQEs. If the batch exceeds ring capacity we chunk.
        fn execute_batch(&mut self, ops: &mut [BatchOp]) {
            if ops.is_empty() {
                return;
            }
            let cap = self.capacity as usize;
            let mut i = 0;
            while i < ops.len() {
                let end = (i + cap).min(ops.len());
                let base = i;
                self.submit_chunk(&mut ops[i..end], base);
                i = end;
            }
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
        std::fs::File::open(&path)
            .unwrap()
            .read_to_end(&mut contents)
            .unwrap();
        assert_eq!(contents, data);
    }
}
