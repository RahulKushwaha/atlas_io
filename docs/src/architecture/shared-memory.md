# Shared Memory Transport

Atlas uses the [`que`](https://crates.io/crates/que) crate for lock-free SPSC (single-producer, single-consumer) ring buffers backed by POSIX shared memory.

## Why Shared Memory?

- **Zero-copy** — No serialization/deserialization overhead; `IoRequest` and `IoResponse` are `AnyBitPattern` and placed directly in the ring
- **Cross-process** — The daemon runs as a separate process for isolation
- **Low latency** — Lock-free push/pop with cache-line-aligned atomics
- **High throughput** — The `que` crate benchmarks at 50M+ messages/second

## Channel Setup

The **client** creates all three shared memory segments on startup:

```rust
let client = AtlasClient::new(instance_id)?;
// Creates: /atlas_req_{id}, /atlas_resp_{id}, /atlas_data_{id}
```

The **service** joins the existing segments:

```rust
let channel = ServiceChannel::join(instance_id)?;
```

## Backpressure

When the request ring is full, the client **spin-waits** until space is available. This preserves RocksDB's synchronous `Env` contract — the calling thread blocks until the I/O completes.

```rust
// Client side
loop {
    match self.req_tx.push(req) {
        Ok(()) => { self.req_tx.sync(); break; }
        Err(_) => std::hint::spin_loop(),
    }
}
```

## Data Region

Large payloads (read/write buffers) don't fit in the ring entries. Instead, the client allocates space in a 64 MB shared data region using a simple atomic bump allocator with wrap-around:

```rust
let offset = self.channel.data.alloc(buf.len());
self.channel.data.write(offset, buf);
// Then reference offset in the IoRequest
```

The service reads/writes directly to/from this region, avoiding any copies.
