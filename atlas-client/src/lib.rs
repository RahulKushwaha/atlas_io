pub mod channel;
pub mod ffi;

use atlas_protocol::{IoOp, IoPriority, IoRequest};
use channel::ClientChannel;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct AtlasClient {
    channel: ClientChannel,
}

impl AtlasClient {
    /// Create a new client. Startup protocol:
    /// 1. `prepare(id)` — creates req ring + data region
    /// 2. Start the service (joins req ring, creates resp ring)
    /// 3. `connect(prepared)` — joins resp ring
    ///
    /// `new()` is a convenience that does prepare + sleep + connect.
    pub fn new(instance_id: u32) -> std::io::Result<Self> {
        Ok(Self { channel: ClientChannel::create(instance_id)? })
    }

    /// Prepare shared memory (req ring + data). Service must start after this.
    pub fn prepare(instance_id: u32) -> std::io::Result<channel::PreparedChannel> {
        channel::PreparedChannel::create(instance_id)
    }

    /// Connect to the service (joins resp ring). Service must be running.
    pub fn connect(prepared: channel::PreparedChannel) -> std::io::Result<Self> {
        Ok(Self { channel: prepared.connect()? })
    }

    pub fn open(&mut self, path: &str, flags: u32) -> Result<u64, i32> {
        let mut req = IoRequest::new(next_id(), IoOp::Open, IoPriority::High);
        req.set_path(path);
        req.len = flags;
        let resp = self.channel.send_request(&req);
        if resp.status != 0 { Err(resp.status) } else { Ok(resp.fd) }
    }

    pub fn read(&mut self, fd: u64, buf: &mut [u8], offset: u64) -> Result<usize, i32> {
        let data_off = self.channel.data.alloc(buf.len());
        let mut req = IoRequest::new(next_id(), IoOp::Read, IoPriority::High);
        req.fd = fd;
        req.offset = offset;
        req.len = buf.len() as u32;
        req.data_offset = data_off;
        let resp = self.channel.send_request(&req);
        if resp.status != 0 { return Err(resp.status); }
        let data = self.channel.data.read(resp.data_offset, resp.data_len as usize);
        buf[..resp.data_len as usize].copy_from_slice(data);
        Ok(resp.data_len as usize)
    }

    pub fn write(&mut self, fd: u64, buf: &[u8], offset: u64) -> Result<usize, i32> {
        let data_off = self.channel.data.alloc(buf.len());
        self.channel.data.write(data_off, buf);
        let mut req = IoRequest::new(next_id(), IoOp::Write, IoPriority::High);
        req.fd = fd;
        req.offset = offset;
        req.len = buf.len() as u32;
        req.data_offset = data_off;
        let resp = self.channel.send_request(&req);
        if resp.status != 0 { Err(resp.status) } else { Ok(resp.data_len as usize) }
    }

    pub fn sync(&mut self, fd: u64) -> Result<(), i32> {
        let mut req = IoRequest::new(next_id(), IoOp::Sync, IoPriority::Critical);
        req.fd = fd;
        let resp = self.channel.send_request(&req);
        if resp.status != 0 { Err(resp.status) } else { Ok(()) }
    }

    pub fn close(&mut self, fd: u64) -> Result<(), i32> {
        let mut req = IoRequest::new(next_id(), IoOp::Close, IoPriority::Low);
        req.fd = fd;
        let resp = self.channel.send_request(&req);
        if resp.status != 0 { Err(resp.status) } else { Ok(()) }
    }
}
