# Atlas Client Library

The `atlas-client` crate provides the Rust API and C FFI for communicating with the Atlas daemon.

## Rust API

```rust
use atlas_client::AtlasClient;

let mut client = AtlasClient::new(instance_id)?;

// File operations
let fd = client.open("/path/to/file.sst", flags)?;
let bytes_read = client.read(fd, &mut buf, offset)?;
let bytes_written = client.write(fd, &data, offset)?;
client.sync(fd)?;
client.close(fd)?;
```

All operations are **synchronous** — the calling thread blocks (spin-waits) until the daemon responds. This matches RocksDB's synchronous `Env` contract.

## How It Works

1. `AtlasClient::new(id)` creates three shared memory segments and connects to the daemon
2. Each method serializes an `IoRequest`, allocates space in the data region if needed, and pushes to the request ring
3. The client then spin-waits on the response ring for a matching response ID
4. The result is returned to the caller
