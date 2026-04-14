# Atlas I/O

Atlas is a centralized I/O service for consolidating file operations across thousands of RocksDB instances running on a single machine.

## The Problem

When thousands of RocksDB instances share a machine, each performs I/O independently — uncoordinated disk access patterns, excessive syscalls, and suboptimal NVMe utilization. Small 4KB point lookups and large multi-MB compaction writes compete for the same device queues without any global awareness.

## The Solution

Atlas interposes a single I/O daemon between all RocksDB instances and the underlying storage:

```
RocksDB Instances (1000s)
  → Custom Env (C++ shim → Rust FFI)
    → Per-instance SPSC Ring Buffer (shared memory)
      → Atlas I/O Service Daemon
        → Batching Engine (coalesce adjacent I/Os)
          → Priority Scheduler (foreground > compaction)
            → I/O Executor (io_uring on Linux, pread/pwrite fallback)
              → Local NVMe SSDs
```

## Key Features

- **Shared-memory transport** — Lock-free SPSC ring buffers via the `que` crate (50M+ msgs/sec)
- **I/O batching** — Adjacent reads/writes to the same file are coalesced into single operations
- **Priority scheduling** — Critical (WAL sync) > High (foreground reads) > Low (compaction)
- **Pluggable executor** — `IoExecutor` trait with `PosixExecutor` and `IoUringExecutor` backends
- **RocksDB integration** — Custom `Env` via C++ shim, intercepting hot-path file ops only
- **Metadata pass-through** — Rename, delete, directory listing go directly to the OS

## Crate Structure

| Crate | Purpose |
|-------|---------|
| `atlas-protocol` | Wire format: `IoRequest`, `IoResponse`, shared constants |
| `atlas-client` | Rust client library + C FFI for the custom Env shim |
| `atlas-service` | The I/O daemon: batching, scheduling, execution |
