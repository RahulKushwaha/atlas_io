# Getting Started

## Prerequisites

- Rust 1.85+ (2024 edition)
- Linux for io_uring support (macOS works with POSIX fallback)
- RocksDB headers (for C++ Env shim compilation)

## Quick Start

```bash
# Clone and build
cd atlas_io
cargo build --release

# Start the daemon for instances 0, 1, 2
cargo run --release -p atlas-service -- 0 1 2

# In your application, create a client
use atlas_client::AtlasClient;
let mut client = AtlasClient::new(0)?;
```

## Minimal Example

```rust
use atlas_client::AtlasClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = AtlasClient::new(0)?;

    // Write
    let fd = client.open("/tmp/atlas_test.dat",
        (libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC) as u32)?;
    client.write(fd, b"Hello, Atlas!", 0)?;
    client.sync(fd)?;
    client.close(fd)?;

    // Read back
    let fd = client.open("/tmp/atlas_test.dat", libc::O_RDONLY as u32)?;
    let mut buf = vec![0u8; 64];
    let n = client.read(fd, &mut buf, 0)?;
    println!("{}", std::str::from_utf8(&buf[..n])?);
    client.close(fd)?;

    Ok(())
}
```
