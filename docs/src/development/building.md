# Building

## Requirements

- Rust 1.85+ (uses edition 2024)
- C++ compiler (for the Env shim, optional)
- RocksDB headers (for the Env shim, optional)

## Build All Crates

```bash
cargo build --release
```

This produces:
- `target/release/atlas-service` — The daemon binary
- `target/release/libatlas_client.a` — Static client library
- `target/release/libatlas_client.{so,dylib}` — Dynamic client library

## Build Individual Crates

```bash
cargo build --release -p atlas-protocol
cargo build --release -p atlas-client
cargo build --release -p atlas-service
```

## Cross-Platform Notes

| Feature | macOS | Linux |
|---------|-------|-------|
| POSIX executor | ✅ | ✅ |
| io_uring executor | ❌ | ✅ |
| Shared memory (que) | ✅ | ✅ |
| Huge pages | ❌ | ✅ |
| C++ Env shim | ✅ | ✅ |

The `io-uring` crate is only compiled on Linux via `cfg(target_os = "linux")`.
