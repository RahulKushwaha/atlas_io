# Priority Scheduler

The scheduler ensures latency-sensitive operations (foreground reads, WAL syncs) are executed before background work (compaction).

## Priority Queues

| Queue | Priority | Typical Operations |
|-------|----------|-------------------|
| Critical | 2 | WAL `fsync` |
| High | 1 | Point lookup reads, foreground writes |
| Low | 0 | Compaction reads/writes |

## Drain Order

The scheduler drains queues strictly in priority order:

```
Critical → High → Low
```

Under SQ pressure (when the io_uring submission queue is full), low-priority compaction I/Os are deferred, preventing them from starving foreground reads.

## Setting Priority

Priority is set by the client based on the RocksDB operation context. The C++ Env shim can tag requests based on whether they originate from compaction, flush, or user reads.
