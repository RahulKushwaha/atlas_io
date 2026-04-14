# RocksDB Env Shim (C++)

The C++ shim subclasses `rocksdb::EnvWrapper` to intercept hot-path file operations and route them through the Atlas daemon. Metadata operations pass through to the default OS Env.

## Intercepted Operations

| RocksDB Method | Atlas Delegation |
|---------------|-----------------|
| `NewRandomAccessFile()` | Returns `AtlasRandomAccessFile` → `atlas_client_open` + `atlas_client_read` |
| `NewWritableFile()` | Returns `AtlasWritableFile` → `atlas_client_open` + `atlas_client_write` + `atlas_client_sync` |

## Pass-through Operations

Everything else delegates to the wrapped default `Env`:
- `DeleteFile`, `RenameFile`, `CreateDir`, `GetChildren`
- `GetFileSize`, `FileExists`, `LockFile`, `UnlockFile`
- Thread management, scheduling, time

## Source Files

```
atlas-client/cpp/
├── atlas_env.h      # Class declarations
└── atlas_env.cpp    # Implementation
```

## Compiling

The C++ shim must be compiled against your RocksDB headers:

```bash
# Build the Rust client library first
cargo build --release -p atlas-client

# Compile the C++ shim
g++ -std=c++17 -c atlas-client/cpp/atlas_env.cpp \
    -I/path/to/rocksdb/include \
    -Iatlas-client/include \
    -o atlas_env.o

# Link into your application with libatlas_client
g++ your_app.cpp atlas_env.o \
    -Ltarget/release -latlas_client \
    -lrocksdb -lpthread -ldl \
    -o your_app
```

## Using from Rust

If using `rust-rocksdb`, you can use `Env::from_raw()`:

```rust
extern "C" { fn create_atlas_env(instance_id: u32) -> *mut std::ffi::c_void; }

let raw_env = unsafe { create_atlas_env(42) };
let env = unsafe { rocksdb::Env::from_raw(raw_env as *mut _) };
let mut opts = rocksdb::Options::default();
opts.set_env(&env);
let db = rocksdb::DB::open(&opts, "/path/to/db").unwrap();
```
