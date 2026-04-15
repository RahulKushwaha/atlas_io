use atlas_protocol::spsc::{Consumer, Producer, SpscRing};
use atlas_protocol::{
    DATA_REGION_SIZE, IoRequest, IoResponse, RING_CAPACITY, shm_data_name, shm_req_name,
    shm_resp_name,
};
use nix::fcntl::OFlag;
use nix::sys::mman::{MapFlags, ProtFlags, mmap, shm_open};
use nix::sys::stat::Mode;
use std::num::NonZero;
use std::os::fd::OwnedFd;

type ReqRing = SpscRing<IoRequest, RING_CAPACITY>;
type RespRing = SpscRing<IoResponse, RING_CAPACITY>;
type ReqConsumer = Consumer<IoRequest, RING_CAPACITY>;
type RespProducer = Producer<IoResponse, RING_CAPACITY>;

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

fn open_existing_ring(name: &str, size: usize) -> std::io::Result<(OwnedFd, *mut u8)> {
    let fd = shm_open(name, OFlag::O_RDWR, Mode::from_bits_truncate(0o600))
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
    let ptr = map_shm_sized(&fd, size)?;
    Ok((fd, ptr))
}

fn create_or_open_ring(name: &str, size: usize) -> std::io::Result<(OwnedFd, *mut u8)> {
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

pub struct DataRegion {
    ptr: *mut u8,
    _fd: OwnedFd,
}

unsafe impl Send for DataRegion {}
unsafe impl Sync for DataRegion {}

impl DataRegion {
    pub fn join(name: &str) -> std::io::Result<Self> {
        let fd = shm_open(name, OFlag::O_RDWR, Mode::from_bits_truncate(0o600))
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        let ptr = unsafe {
            mmap(
                None,
                NonZero::new(DATA_REGION_SIZE).unwrap(),
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                &fd,
                0,
            )
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?
        };
        Ok(Self {
            ptr: ptr.as_ptr() as *mut u8,
            _fd: fd,
        })
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

pub struct ServiceChannel {
    req_rx: ReqConsumer,
    _req_fd: OwnedFd,
    resp_tx: RespProducer,
    _resp_fd: OwnedFd,
    pub data: DataRegion,
    pub instance_id: u32,
}

impl ServiceChannel {
    pub fn join(instance_id: u32) -> std::io::Result<Self> {
        let req_name = shm_req_name(instance_id);
        let resp_name = shm_resp_name(instance_id);
        let data_name = shm_data_name(instance_id);

        // Request ring was created by the client in `PreparedChannel::create`.
        let (req_fd, req_ptr) = open_existing_ring(&req_name, ReqRing::size_bytes())?;
        let req_ring = unsafe { ReqRing::attach(req_ptr) }
            .map_err(|e| std::io::Error::other(format!("req ring attach: {e}")))?;
        let req_rx = unsafe { ReqConsumer::new(req_ring) };

        // Response ring is created by the service and joined by the client later.
        let (resp_fd, resp_ptr) = create_or_open_ring(&resp_name, RespRing::size_bytes())?;
        let resp_ring = unsafe { RespRing::init_in_place(resp_ptr) };
        let resp_tx = unsafe { RespProducer::new(resp_ring) };

        let data = DataRegion::join(&data_name)?;

        Ok(Self {
            req_rx,
            _req_fd: req_fd,
            resp_tx,
            _resp_fd: resp_fd,
            data,
            instance_id,
        })
    }

    pub fn poll_request(&mut self) -> Option<IoRequest> {
        self.req_rx.pop()
    }

    pub fn send_response(&mut self, resp: &IoResponse) {
        loop {
            match self.resp_tx.push(resp) {
                Ok(()) => return,
                Err(_) => std::thread::yield_now(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_client::channel::{PreparedChannel, cleanup_shm};
    use atlas_protocol::{IoOp, IoPriority, IoRequest, IoResponse};

    fn setup(id: u32) -> (atlas_client::channel::ClientChannel, ServiceChannel) {
        cleanup_shm(id);
        let prepared = PreparedChannel::create(id).expect("prepare");
        let svc_ch = ServiceChannel::join(id).expect("service join");
        let client_ch = prepared.connect().expect("client connect");
        (client_ch, svc_ch)
    }

    #[test]
    fn poll_request_empty() {
        let id = 6000;
        let (_client, mut svc) = setup(id);
        assert!(svc.poll_request().is_none());
        cleanup_shm(id);
    }

    #[test]
    fn request_roundtrip() {
        let id = 6001;
        let (mut client, mut svc) = setup(id);

        let handle = std::thread::spawn(move || {
            let mut req = IoRequest::new(42, IoOp::Open, IoPriority::High);
            req.set_path("/tmp/test.dat");
            client.send_request(&req)
        });

        let req = loop {
            if let Some(r) = svc.poll_request() {
                break r;
            }
            std::thread::yield_now();
        };
        assert_eq!(req.id, 42);
        assert_eq!(req.io_op(), Some(IoOp::Open));
        assert_eq!(req.path_str(), "/tmp/test.dat");

        let mut resp = IoResponse::ok(42);
        resp.fd = 7;
        svc.send_response(&resp);

        let got = handle.join().unwrap();
        assert_eq!(got.id, 42);
        assert_eq!(got.fd, 7);
        assert_eq!(got.status, 0);
        cleanup_shm(id);
    }

    #[test]
    fn multiple_request_response_cycles() {
        let id = 6002;
        let (mut client, mut svc) = setup(id);

        let handle = std::thread::spawn(move || {
            for i in 0..10u64 {
                let req = IoRequest::new(i, IoOp::Read, IoPriority::High);
                let resp = client.send_request(&req);
                assert_eq!(resp.id, i);
                assert_eq!(resp.status, 0);
            }
        });

        for _ in 0..10 {
            let req = loop {
                if let Some(r) = svc.poll_request() {
                    break r;
                }
                std::thread::yield_now();
            };
            svc.send_response(&IoResponse::ok(req.id));
        }

        handle.join().unwrap();
        cleanup_shm(id);
    }

    #[test]
    fn data_region_shared_between_client_and_service() {
        let id = 6003;
        let (client, svc) = setup(id);

        client.data.write(0, b"from_client");
        assert_eq!(svc.data.read(0, 11), b"from_client");

        svc.data.write(64, b"from_service");
        assert_eq!(client.data.read(64, 12), b"from_service");
        cleanup_shm(id);
    }

    #[test]
    fn error_response() {
        let id = 6004;
        let (mut client, mut svc) = setup(id);

        let handle = std::thread::spawn(move || {
            let req = IoRequest::new(99, IoOp::Open, IoPriority::High);
            client.send_request(&req)
        });

        let req = loop {
            if let Some(r) = svc.poll_request() {
                break r;
            }
            std::thread::yield_now();
        };
        svc.send_response(&IoResponse::err(req.id, -libc::ENOENT));

        let resp = handle.join().unwrap();
        assert_eq!(resp.id, 99);
        assert_eq!(resp.status, -libc::ENOENT);
        cleanup_shm(id);
    }
}
