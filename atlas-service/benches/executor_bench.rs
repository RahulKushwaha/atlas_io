use atlas_service::executor::{IoExecutor, PosixExecutor};
use criterion::{Criterion, BenchmarkId, criterion_group, criterion_main, Throughput};
use std::os::fd::RawFd;

fn setup_file(ex: &mut PosixExecutor, dir: &tempfile::TempDir, size: usize) -> (RawFd, String) {
    let path = dir.path().join(format!("bench_{size}.dat"));
    let path_str = path.to_str().unwrap().to_string();
    let fd = ex.open(&path_str, libc::O_CREAT | libc::O_RDWR, 0o644) as RawFd;
    // Pre-fill file
    let buf = vec![0xABu8; size];
    ex.pwrite(fd, buf.as_ptr(), size, 0);
    ex.fsync(fd);
    (fd, path_str)
}

fn bench_pread(c: &mut Criterion) {
    let mut group = c.benchmark_group("posix_pread");
    let dir = tempfile::tempdir().unwrap();
    let mut ex = PosixExecutor::new();

    for size in [4096, 16384, 65536, 262144] {
        let (fd, _path) = setup_file(&mut ex, &dir, size);
        let mut buf = vec![0u8; size];

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &sz| {
            b.iter(|| {
                ex.pread(fd, buf.as_mut_ptr(), sz, 0);
            });
        });
        ex.close(fd);
    }
    group.finish();
}

fn bench_pwrite(c: &mut Criterion) {
    let mut group = c.benchmark_group("posix_pwrite");
    let dir = tempfile::tempdir().unwrap();
    let mut ex = PosixExecutor::new();

    for size in [4096, 16384, 65536, 262144] {
        let (fd, _path) = setup_file(&mut ex, &dir, size);
        let buf = vec![0xCDu8; size];

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &sz| {
            b.iter(|| {
                ex.pwrite(fd, buf.as_ptr(), sz, 0);
            });
        });
        ex.close(fd);
    }
    group.finish();
}

fn bench_fsync(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let mut ex = PosixExecutor::new();
    let (fd, _path) = setup_file(&mut ex, &dir, 4096);

    c.bench_function("posix_fsync", |b| {
        b.iter(|| {
            ex.fsync(fd);
        });
    });
    ex.close(fd);
}

fn bench_open_close(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let mut ex = PosixExecutor::new();
    let path = dir.path().join("open_close.dat");
    let path_str = path.to_str().unwrap();
    // Create the file first
    let fd = ex.open(path_str, libc::O_CREAT | libc::O_RDWR, 0o644) as RawFd;
    ex.close(fd);

    c.bench_function("posix_open_close", |b| {
        b.iter(|| {
            let fd = ex.open(path_str, libc::O_RDONLY, 0) as RawFd;
            ex.close(fd);
        });
    });
}

fn bench_sequential_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("posix_sequential_write");
    let dir = tempfile::tempdir().unwrap();
    let mut ex = PosixExecutor::new();

    let chunk = 4096;
    for num_chunks in [16, 64, 256] {
        let total = chunk * num_chunks;
        let path = dir.path().join(format!("seq_{num_chunks}.dat"));
        let path_str = path.to_str().unwrap().to_string();
        let buf = vec![0xEFu8; chunk];

        group.throughput(Throughput::Bytes(total as u64));
        group.bench_with_input(BenchmarkId::from_parameter(num_chunks), &num_chunks, |b, &n| {
            b.iter(|| {
                let fd = ex.open(&path_str, libc::O_CREAT | libc::O_RDWR | libc::O_TRUNC, 0o644) as RawFd;
                for i in 0..n {
                    ex.pwrite(fd, buf.as_ptr(), chunk, (i * chunk) as u64);
                }
                ex.fsync(fd);
                ex.close(fd);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_pread, bench_pwrite, bench_fsync, bench_open_close, bench_sequential_write);
criterion_main!(benches);
