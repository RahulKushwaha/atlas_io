# I/O Executor Trait

The `IoExecutor` trait abstracts the low-level I/O backend, allowing the service to swap between synchronous POSIX calls and Linux's io_uring.

## Trait Definition

```rust
pub trait IoExecutor {
    fn open(&mut self, path: &str, flags: i32, mode: u32) -> IoResult;
    fn close(&mut self, fd: RawFd) -> IoResult;
    fn pread(&mut self, fd: RawFd, buf: *mut u8, len: usize, offset: u64) -> IoResult;
    fn pwrite(&mut self, fd: RawFd, buf: *const u8, len: usize, offset: u64) -> IoResult;
    fn fsync(&mut self, fd: RawFd) -> IoResult;
}
```

`IoResult` is `i64` — positive values are bytes transferred (or fd for open), negative values are `-errno`.

## PosixExecutor

Available on all platforms. Wraps `libc::pread`, `libc::pwrite`, `libc::fsync` directly.

```rust
let mut executor = PosixExecutor::new();
```

## IoUringExecutor (Linux only)

Uses the `io-uring` crate (tokio-rs). Each operation submits a single SQE and waits for the CQE. Available behind `#[cfg(target_os = "linux")]`.

```rust
let mut executor = IoUringExecutor::new(256)?; // 256 SQ entries
```

Supported opcodes:
- `opcode::OpenAt` for open
- `opcode::Close` for close
- `opcode::Read` for pread
- `opcode::Write` for pwrite
- `opcode::Fsync` for fsync

## Selecting at Runtime

The daemon selects the executor based on the `ATLAS_USE_URING` environment variable (Linux only):

```bash
# Use io_uring
ATLAS_USE_URING=1 atlas-service 0 1 2

# Use POSIX fallback (default)
atlas-service 0 1 2
```
