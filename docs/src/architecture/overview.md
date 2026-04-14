# Architecture Overview

## Data Flow

A single I/O request follows this path:

1. **RocksDB** calls `Read()` or `Append()` on a file handle
2. **C++ Env shim** translates this into an `atlas_client_read` / `atlas_client_write` FFI call
3. **AtlasClient** (Rust) serializes the request into an `IoRequest`, writes any payload into the shared data region, and pushes the request onto the SPSC ring buffer
4. The client **spin-waits** for a matching `IoResponse` on the response ring
5. **AtlasService** (daemon) polls the request ring, collects pending ops into the **BatchEngine**
6. The BatchEngine **merges adjacent** reads/writes on the same file descriptor
7. Merged ops are fed into the **Scheduler** which orders them by priority
8. The **IoExecutor** (PosixExecutor or IoUringExecutor) performs the actual I/O
9. Responses are pushed back through the response ring to the waiting client

## Process Model

```
┌─────────────────────┐     ┌─────────────────────┐
│  RocksDB Process A  │     │  RocksDB Process B  │
│  ┌───────────────┐  │     │  ┌───────────────┐  │
│  │ Custom Env    │  │     │  │ Custom Env    │  │
│  │ → AtlasClient │  │     │  │ → AtlasClient │  │
│  └──────┬────────┘  │     │  └──────┬────────┘  │
└─────────┼───────────┘     └─────────┼───────────┘
          │ shared memory             │ shared memory
          ▼                           ▼
┌─────────────────────────────────────────────────┐
│              Atlas I/O Service Daemon            │
│  ┌──────────┐  ┌──────────┐  ┌───────────────┐ │
│  │ Channels │→ │ Batcher  │→ │  Scheduler    │ │
│  └──────────┘  └──────────┘  └───────┬───────┘ │
│                                      ▼         │
│                              ┌───────────────┐ │
│                              │  IoExecutor   │ │
│                              │ (posix/uring) │ │
│                              └───────┬───────┘ │
└──────────────────────────────────────┼─────────┘
                                       ▼
                                   NVMe SSDs
```

## Shared Memory Layout (per instance)

Each RocksDB instance gets three shared memory segments:

| Segment | Name | Size | Purpose |
|---------|------|------|---------|
| Request ring | `/atlas_req_{id}` | ~1.2 MB | SPSC queue of `IoRequest` (296 bytes × 4096 slots) |
| Response ring | `/atlas_resp_{id}` | ~128 KB | SPSC queue of `IoResponse` (32 bytes × 4096 slots) |
| Data region | `/atlas_data_{id}` | 64 MB | Bulk read/write payload buffer |

The client is the producer on the request ring and consumer on the response ring. The service is the inverse.
