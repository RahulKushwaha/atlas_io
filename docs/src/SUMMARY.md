# Summary

[Introduction](./introduction.md)

# Architecture

- [Overview](./architecture/overview.md)
- [Protocol](./architecture/protocol.md)
- [Shared Memory Transport](./architecture/shared-memory.md)

# Components

- [Atlas Service (Daemon)](./components/service.md)
  - [I/O Executor Trait](./components/executor.md)
  - [Batching Engine](./components/batching.md)
  - [Priority Scheduler](./components/scheduler.md)
- [Atlas Client Library](./components/client.md)
  - [C FFI](./components/ffi.md)
- [RocksDB Env Shim (C++)](./components/env-shim.md)

# Usage

- [Getting Started](./usage/getting-started.md)
- [Running the Daemon](./usage/running.md)
- [Integrating with RocksDB](./usage/rocksdb-integration.md)
- [Configuration & Tuning](./usage/tuning.md)

# Development

- [Building](./development/building.md)
- [Testing](./development/testing.md)
- [Benchmarking](./development/benchmarking.md)
