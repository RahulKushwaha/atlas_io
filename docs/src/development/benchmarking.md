# Benchmarking

## Running Benchmarks

```bash
cargo bench -p atlas-service --bench executor_bench
```

Results are saved to `target/criterion/` with HTML reports.

## Benchmark Suites

### `posix_pread`

Sequential pread at offset 0 for varying buffer sizes (4KB, 16KB, 64KB, 256KB). Measures raw read syscall throughput.

### `posix_pwrite`

Sequential pwrite at offset 0 for varying buffer sizes. Measures raw write syscall throughput.

### `posix_fsync`

Fsync on a small file. On macOS this often hits the page cache; on Linux with real NVMe this is the true device flush latency.

### `posix_open_close`

Open + close cycle on an existing file. This is typically the most expensive single operation (~200µs) and demonstrates why fd reuse through the daemon is valuable.

### `posix_sequential_write`

Simulates a realistic write workload: open → N × 4KB pwrite → fsync → close. Tested with 16, 64, and 256 chunks (64KB, 256KB, 1MB total).

## Interpreting Results

Key things to look for:

- **pread/pwrite throughput** should scale with buffer size up to the memory bandwidth limit
- **fsync latency** on real NVMe is typically 10-100µs (vs ~300ns on macOS page cache)
- **open+close** overhead justifies the daemon's fd pooling
- **sequential write** throughput shows the combined cost of many small writes + fsync

## Adding io_uring Benchmarks

On Linux, you can add a parallel benchmark group for `IoUringExecutor` to compare:

```rust
#[cfg(target_os = "linux")]
fn bench_uring_pread(c: &mut Criterion) {
    let mut ex = IoUringExecutor::new(256).unwrap();
    // ... same structure as posix benchmarks
}
```
