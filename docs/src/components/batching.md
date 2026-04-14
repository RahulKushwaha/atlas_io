# Batching Engine

The `BatchEngine` coalesces adjacent I/O operations on the same file descriptor into single larger operations, reducing syscall overhead.

## How It Works

1. During the drain phase, all pending requests are collected into the batch engine
2. Requests are separated by type: reads, writes, and non-mergeable ops (open/close/sync)
3. Reads and writes are sorted by `(channel_idx, fd, offset)`
4. Adjacent or overlapping operations on the same fd are merged into a single operation

## Example

Two 4KB reads at adjacent offsets:

```
Request 1: Read { fd=10, offset=0,    len=4096 }
Request 2: Read { fd=10, offset=4096, len=4096 }
```

Become a single 8KB read:

```
Merged:    Read { fd=10, offset=0, len=8192 }
```

The response is then split back — each original requestor gets their slice of the data.

## What Gets Merged

- ✅ Reads on the same fd with adjacent/overlapping offsets
- ✅ Writes on the same fd with adjacent/overlapping offsets
- ❌ Operations on different fds
- ❌ Operations from different channels (instances)
- ❌ Open, Close, Sync (pass through as-is)
