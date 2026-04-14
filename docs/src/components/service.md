# Atlas Service (Daemon)

The `atlas-service` crate is the centralized I/O daemon. It runs as a standalone process and serves file I/O requests from all connected RocksDB instances.

## Main Loop

The service runs a tight poll loop with three phases:

```
Phase 1: DRAIN    — Poll all ServiceChannels for pending requests
Phase 2: BATCH    — Merge adjacent reads/writes via BatchEngine
Phase 3: EXECUTE  — Schedule by priority, execute via IoExecutor
```

```rust
while running.load(Ordering::Relaxed) {
    // Phase 1: Drain
    for i in 0..channels.len() {
        while let Some(req) = channels[i].poll_request() {
            batch.add(req, i);
        }
    }
    // Phase 2: Batch + Schedule
    let merged = batch.drain_merged();
    scheduler.enqueue_all(merged);
    // Phase 3: Execute
    let ops = scheduler.drain(scheduler.len());
    for op in ops {
        execute(op);  // dispatches to IoExecutor
    }
}
```

## Virtual File Descriptors

The service maintains a mapping from virtual fds (returned to clients) to real OS file descriptors. This provides:

- **Isolation** — Clients can't access arbitrary fds
- **Lifecycle management** — The service owns all real fds
- **Cross-process safety** — Virtual fds are just `u64` integers

## `AtlasService<E: IoExecutor>`

The service is generic over the I/O backend:

```rust
// macOS / fallback
let svc = AtlasService::new(running, PosixExecutor::new());

// Linux with io_uring
let svc = AtlasService::new(running, IoUringExecutor::new(256)?);
```
