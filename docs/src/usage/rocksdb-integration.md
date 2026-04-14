# Integrating with RocksDB

## Overview

Atlas integrates with RocksDB through a custom `Env` implementation. The C++ shim (`AtlasEnv`) subclasses `rocksdb::EnvWrapper` and intercepts file read/write/sync operations while letting metadata operations pass through to the OS.

## Step-by-Step

### 1. Build the Atlas client library

```bash
cargo build --release -p atlas-client
# Produces: target/release/libatlas_client.{a,so,dylib}
```

### 2. Compile the C++ Env shim

```bash
g++ -std=c++17 -c atlas-client/cpp/atlas_env.cpp \
    -I/path/to/rocksdb/include \
    -Iatlas-client/include \
    -o atlas_env.o
```

### 3. Start the Atlas daemon

```bash
atlas-service 0 1 2  # one ID per RocksDB instance
```

### 4. Open RocksDB with the custom Env

```cpp
#include "atlas_env.h"

// Create the custom env for instance 0
auto* env = new atlas::AtlasEnv(atlas_client_create(0));

rocksdb::Options options;
options.env = env;
options.create_if_missing = true;

rocksdb::DB* db;
rocksdb::Status s = rocksdb::DB::Open(options, "/path/to/db", &db);
```

## What Gets Intercepted

Only the hot-path I/O operations go through Atlas:

- **SST file reads** (point lookups, range scans) → `AtlasRandomAccessFile::Read()`
- **SST file writes** (flush, compaction output) → `AtlasWritableFile::Append()`
- **WAL writes** → `AtlasWritableFile::Append()`
- **Sync/Fsync** → `AtlasWritableFile::Sync()`

Everything else (file creation metadata, directory operations, locks) uses the default OS path.
