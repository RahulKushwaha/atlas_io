# Configuration & Tuning

## Compile-Time Constants

These are defined in `atlas-protocol/src/lib.rs`:

| Constant | Default | Description |
|----------|---------|-------------|
| `RING_CAPACITY` | 4096 | Slots per SPSC ring buffer |
| `DATA_REGION_SIZE` | 64 MiB | Shared data region per instance |
| `MAX_PATH_LEN` | 256 | Max file path length in requests |

## Tuning Guidelines

### Ring Capacity

- **Increase** if you see high spin-wait contention (client blocking on full ring)
- Must be a power of 2 for the `que` crate
- 4096 slots × 296 bytes ≈ 1.2 MB per instance for the request ring

### Data Region Size

- Must be large enough to hold all in-flight read/write payloads
- With 4096 in-flight requests of 16KB each = 64 MB (the default)
- **Increase** for workloads with many large concurrent reads

### io_uring Queue Depth

- Set via `IoUringExecutor::new(entries)` — default 256
- Higher values allow more in-flight I/Os but consume more kernel memory
- For NVMe with high queue depth, 1024+ may help

### Linux Huge Pages

The `que` crate supports huge pages for TLB efficiency. To enable:

```bash
# Mount hugepages
mount -t hugetlbfs none /dev/hugepages

# Allocate 2MB pages (16 pages = 32MB)
echo 16 > /proc/sys/vm/nr_hugepages
```

Then modify the channel creation to use `PageSize::Huge` (Linux only).
