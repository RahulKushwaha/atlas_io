use bytemuck::{AnyBitPattern, NoUninit};

pub const RING_CAPACITY: usize = 4096;
pub const DATA_REGION_SIZE: usize = 64 * 1024 * 1024; // 64 MiB per instance
pub const MAX_PATH_LEN: usize = 256;
pub const SHM_PREFIX: &str = "atlas";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IoOp {
    Open = 0,
    Read = 1,
    Write = 2,
    Sync = 3,
    Close = 4,
}

impl IoOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Open),
            1 => Some(Self::Read),
            2 => Some(Self::Write),
            3 => Some(Self::Sync),
            4 => Some(Self::Close),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum IoPriority {
    Low = 0,       // compaction
    High = 1,      // foreground reads
    Critical = 2,  // WAL sync
}

impl IoPriority {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Low),
            1 => Some(Self::High),
            2 => Some(Self::Critical),
            _ => None,
        }
    }
}

/// Wire format for I/O requests. All fields are plain integers for AnyBitPattern compat.
#[derive(Debug, Clone, Copy, AnyBitPattern, NoUninit)]
#[repr(C)]
pub struct IoRequest {
    pub id: u64,
    pub op: u8,       // IoOp as u8
    pub priority: u8, // IoPriority as u8
    pub _pad: [u8; 6],
    pub fd: u64,
    pub offset: u64,
    pub len: u32,
    pub data_offset: u32,
    pub path: [u8; MAX_PATH_LEN],
}

#[derive(Debug, Clone, Copy, AnyBitPattern, NoUninit)]
#[repr(C)]
pub struct IoResponse {
    pub id: u64,
    pub status: i32,
    pub _pad: [u8; 4],
    pub fd: u64,
    pub data_offset: u32,
    pub data_len: u32,
}

impl IoRequest {
    pub fn new(id: u64, op: IoOp, priority: IoPriority) -> Self {
        Self {
            id,
            op: op as u8,
            priority: priority as u8,
            _pad: [0; 6],
            fd: 0,
            offset: 0,
            len: 0,
            data_offset: 0,
            path: [0; MAX_PATH_LEN],
        }
    }

    pub fn io_op(&self) -> Option<IoOp> {
        IoOp::from_u8(self.op)
    }

    pub fn io_priority(&self) -> Option<IoPriority> {
        IoPriority::from_u8(self.priority)
    }

    pub fn set_path(&mut self, p: &str) {
        let bytes = p.as_bytes();
        let n = bytes.len().min(MAX_PATH_LEN - 1);
        self.path[..n].copy_from_slice(&bytes[..n]);
        self.path[n] = 0;
    }

    pub fn path_str(&self) -> &str {
        let end = self.path.iter().position(|&b| b == 0).unwrap_or(MAX_PATH_LEN);
        core::str::from_utf8(&self.path[..end]).unwrap_or("")
    }
}

impl IoResponse {
    pub fn ok(id: u64) -> Self {
        Self { id, status: 0, _pad: [0; 4], fd: 0, data_offset: 0, data_len: 0 }
    }

    pub fn err(id: u64, status: i32) -> Self {
        Self { id, status, _pad: [0; 4], fd: 0, data_offset: 0, data_len: 0 }
    }
}

pub fn shm_req_name(instance_id: u32) -> String {
    format!("/{SHM_PREFIX}_req_{instance_id}")
}

pub fn shm_resp_name(instance_id: u32) -> String {
    format!("/{SHM_PREFIX}_resp_{instance_id}")
}

pub fn shm_data_name(instance_id: u32) -> String {
    format!("/{SHM_PREFIX}_data_{instance_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_is_any_bit_pattern() {
        let bytes = [0u8; size_of::<IoRequest>()];
        let _req: &IoRequest = bytemuck::from_bytes(&bytes);
    }

    #[test]
    fn response_is_any_bit_pattern() {
        let bytes = [0u8; size_of::<IoResponse>()];
        let _resp: &IoResponse = bytemuck::from_bytes(&bytes);
    }

    #[test]
    fn sizes_are_stable() {
        assert_eq!(size_of::<IoRequest>(), 296);
        assert_eq!(size_of::<IoResponse>(), 32);
    }

    #[test]
    fn path_roundtrip() {
        let mut req = IoRequest::new(1, IoOp::Open, IoPriority::High);
        req.set_path("/tmp/test.sst");
        assert_eq!(req.path_str(), "/tmp/test.sst");
    }

    #[test]
    fn op_roundtrip() {
        let req = IoRequest::new(1, IoOp::Write, IoPriority::Critical);
        assert_eq!(req.io_op(), Some(IoOp::Write));
        assert_eq!(req.io_priority(), Some(IoPriority::Critical));
    }
}
