use atlas_protocol::spsc::{Consumer, Producer, SpscRing};
use atlas_protocol::{
    DATA_REGION_SIZE, IoRequest, IoResponse, RING_CAPACITY, shm_data_name, shm_req_name,
    shm_resp_name,
};
use nix::fcntl::OFlag;
use nix::sys::mman::{MapFlags, ProtFlags, mmap, shm_open, shm_unlink};
use nix::sys::stat::Mode;
use std::num::NonZero;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU32, Ordering};

type ReqRing = SpscRing<IoRequest, RING_CAPACITY>;
type RespRing = SpscRing<IoResponse, RING_CAPACITY>;
type ReqProducer = Producer<IoRequest, RING_CAPACITY>;
type RespConsumer = Consumer<IoResponse, RING_CAPACITY>;

fn map_shm_sized(fd: &OwnedFd, size: usize) -> std::io::Result<*mut u8> {
    let ptr = unsafe {
        mmap(
            None,
            NonZero::new(size).unwrap(),
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
            MapFlags::MAP_SHARED,
            fd,
            0,
        )
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?
    };
    Ok(ptr.as_ptr() as *mut u8)
}

fn create_ring_shm(name: &str, size: usize) -> std::io::Result<(OwnedFd, *mut u8)> {
    let _ = shm_unlink(name);
    let fd = shm_open(
        name,
        OFlag::O_CREAT | OFlag::O_RDWR,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
    nix::unistd::ftruncate(&fd, size as i64)
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
    let ptr = map_shm_sized(&fd, size)?;
    Ok((fd, ptr))
}

fn open_ring_shm(name: &str, size: usize) -> std::io::Result<(OwnedFd, *mut u8)> {
    let fd = shm_open(name, OFlag::O_RDWR, Mode::from_bits_truncate(0o600))
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
    let ptr = map_shm_sized(&fd, size)?;
    Ok((fd, ptr))
}

pub struct DataRegion {
    ptr: *mut u8,
    len: usize,
    offset: AtomicU32,
    _fd: OwnedFd,
}

unsafe impl Send for DataRegion {}
unsafe impl Sync for DataRegion {}

impl DataRegion {
    pub fn create(name: &str) -> std::io::Result<Self> {
        let _ = shm_unlink(name);
        let fd = shm_open(
            name,
            OFlag::O_CREAT | OFlag::O_RDWR,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        nix::unistd::ftruncate(&fd, DATA_REGION_SIZE as i64)
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        let ptr = map_shm_sized(&fd, DATA_REGION_SIZE)?;
        Ok(Self {
            ptr,
            len: DATA_REGION_SIZE,
            offset: AtomicU32::new(0),
            _fd: fd,
        })
    }

    pub fn join(name: &str) -> std::io::Result<Self> {
        let fd = shm_open(name, OFlag::O_RDWR, Mode::from_bits_truncate(0o600))
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        let ptr = map_shm_sized(&fd, DATA_REGION_SIZE)?;
        Ok(Self {
            ptr,
            len: DATA_REGION_SIZE,
            offset: AtomicU32::new(0),
            _fd: fd,
        })
    }

    pub fn alloc(&self, n: usize) -> u32 {
        let n = n as u32;
        loop {
            let cur = self.offset.load(Ordering::Relaxed);
            let (start, next) = if cur + n > self.len as u32 {
                (0, n)
            } else {
                (cur, cur + n)
            };
            if self
                .offset
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return start;
            }
        }
    }

    pub fn write(&self, offset: u32, data: &[u8]) {
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.add(offset as usize), data.len())
        }
    }

    pub fn read(&self, offset: u32, len: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.add(offset as usize), len) }
    }

    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }
}

pub fn cleanup_shm(instance_id: u32) {
    let _ = shm_unlink(shm_req_name(instance_id).as_str());
    let _ = shm_unlink(shm_resp_name(instance_id).as_str());
    let _ = shm_unlink(shm_data_name(instance_id).as_str());
}

/// Phase 1 result: req ring + data region created, waiting for service to create resp ring.
pub struct PreparedChannel {
    req_tx: ReqProducer,
    _req_fd: OwnedFd,
    data: DataRegion,
    instance_id: u32,
}

impl PreparedChannel {
    pub fn create(instance_id: u32) -> std::io::Result<Self> {
        cleanup_shm(instance_id);
        let (req_fd, req_ptr) = create_ring_shm(&shm_req_name(instance_id), ReqRing::size_bytes())?;
        let ring = unsafe { ReqRing::init_in_place(req_ptr) };
        let req_tx = unsafe { ReqProducer::new(ring) };
        let data = DataRegion::create(&shm_data_name(instance_id))?;
        Ok(Self {
            req_tx,
            _req_fd: req_fd,
            data,
            instance_id,
        })
    }

    /// Phase 2: join the response ring (service must be running).
    pub fn connect(self) -> std::io::Result<ClientChannel> {
        let resp_name = shm_resp_name(self.instance_id);
        let mut last_err = String::new();
        for _ in 0..200 {
            match open_ring_shm(&resp_name, RespRing::size_bytes()) {
                Ok((fd, ptr)) => match unsafe { RespRing::attach(ptr) } {
                    Ok(ring) => {
                        let resp_rx = unsafe { RespConsumer::new(ring) };
                        return Ok(ClientChannel {
                            req_tx: self.req_tx,
                            _req_fd: self._req_fd,
                            resp_rx,
                            _resp_fd: fd,
                            data: self.data,
                            instance_id: self.instance_id,
                        });
                    }
                    Err(e) => last_err = e.to_string(),
                },
                Err(e) => last_err = e.to_string(),
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err(std::io::Error::other(format!(
            "timeout joining resp ring: {last_err}"
        )))
    }
}

pub struct ClientChannel {
    req_tx: ReqProducer,
    _req_fd: OwnedFd,
    resp_rx: RespConsumer,
    _resp_fd: OwnedFd,
    pub data: DataRegion,
    instance_id: u32,
}

impl ClientChannel {
    /// Convenience: prepare + connect (for when service is already running).
    pub fn create(instance_id: u32) -> std::io::Result<Self> {
        let prepared = PreparedChannel::create(instance_id)?;
        prepared.connect()
    }

    pub fn send_request(&mut self, req: &IoRequest) -> IoResponse {
        loop {
            match self.req_tx.push(req) {
                Ok(()) => break,
                Err(_) => std::thread::yield_now(),
            }
        }
        loop {
            if let Some(resp) = self.resp_rx.pop() {
                if resp.id == req.id {
                    return resp;
                }
            }
            std::thread::yield_now();
        }
    }

    pub fn instance_id(&self) -> u32 {
        self.instance_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_protocol::DATA_REGION_SIZE;

    fn create_region(name: &str) -> DataRegion {
        DataRegion::create(name).expect("create data region")
    }

    #[test]
    fn write_read_roundtrip() {
        let dr = create_region("/atlas_test_wr_roundtrip");
        let data = b"hello atlas";
        let off = dr.alloc(data.len());
        dr.write(off, data);
        assert_eq!(dr.read(off, data.len()), data);
        let _ = shm_unlink("/atlas_test_wr_roundtrip");
    }

    #[test]
    fn alloc_advances_offset() {
        let dr = create_region("/atlas_test_alloc_adv");
        let a = dr.alloc(100);
        let b = dr.alloc(200);
        assert_eq!(a, 0);
        assert_eq!(b, 100);
        let c = dr.alloc(50);
        assert_eq!(c, 300);
        let _ = shm_unlink("/atlas_test_alloc_adv");
    }

    #[test]
    fn alloc_wraps_around() {
        let dr = create_region("/atlas_test_alloc_wrap");
        let big = DATA_REGION_SIZE - 16;
        let off1 = dr.alloc(big);
        assert_eq!(off1, 0);
        let off2 = dr.alloc(32);
        assert_eq!(off2, 0);
        let _ = shm_unlink("/atlas_test_alloc_wrap");
    }

    #[test]
    fn non_overlapping_writes() {
        let dr = create_region("/atlas_test_nonoverlap");
        let a = dr.alloc(4);
        let b = dr.alloc(4);
        dr.write(a, b"AAAA");
        dr.write(b, b"BBBB");
        assert_eq!(dr.read(a, 4), b"AAAA");
        assert_eq!(dr.read(b, 4), b"BBBB");
        let _ = shm_unlink("/atlas_test_nonoverlap");
    }

    #[test]
    fn join_sees_written_data() {
        let name = "/atlas_test_join_sees";
        let dr = create_region(name);
        let off = dr.alloc(5);
        dr.write(off, b"share");

        let dr2 = DataRegion::join(name).expect("join");
        assert_eq!(dr2.read(off, 5), b"share");
        let _ = shm_unlink(name);
    }
}
