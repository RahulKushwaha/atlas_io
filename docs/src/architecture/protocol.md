# Protocol

All communication between client and service uses fixed-size, zero-copy structs that implement `AnyBitPattern` from the `bytemuck` crate.

## IoRequest (296 bytes)

```rust
#[repr(C)]
pub struct IoRequest {
    pub id: u64,          // Unique request ID
    pub op: u8,           // IoOp discriminant
    pub priority: u8,     // IoPriority discriminant
    pub _pad: [u8; 6],
    pub fd: u64,          // Virtual file descriptor
    pub offset: u64,      // File offset for read/write
    pub len: u32,         // Byte count (or flags for Open)
    pub data_offset: u32, // Offset into shared data region
    pub path: [u8; 256],  // Null-terminated path (Open only)
}
```

## IoResponse (32 bytes)

```rust
#[repr(C)]
pub struct IoResponse {
    pub id: u64,          // Matching request ID
    pub status: i32,      // 0 = success, negative = -errno
    pub _pad: [u8; 4],
    pub fd: u64,          // Virtual fd (Open response only)
    pub data_offset: u32, // Where to find read data
    pub data_len: u32,    // Bytes actually read/written
}
```

## Operations

| Op | `op` value | Request fields used | Response fields used |
|----|-----------|-------------------|---------------------|
| Open | 0 | `path`, `len` (flags) | `fd` |
| Read | 1 | `fd`, `offset`, `len`, `data_offset` | `data_offset`, `data_len` |
| Write | 2 | `fd`, `offset`, `len`, `data_offset` | `data_len` |
| Sync | 3 | `fd` | `status` |
| Close | 4 | `fd` | `status` |

## Priority Levels

| Priority | Value | Use case |
|----------|-------|----------|
| Low | 0 | Compaction I/O |
| High | 1 | Foreground reads (point lookups) |
| Critical | 2 | WAL sync |
