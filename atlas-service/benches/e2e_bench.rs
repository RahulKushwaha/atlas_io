use atlas_client::channel::cleanup_shm;
use atlas_client::AtlasClient;
use atlas_service::executor::{IoExecutor, PosixExecutor};
use atlas_service::service::AtlasService;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn setup_pair(id: u32) -> (AtlasClient, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    cleanup_shm(id);
    let prepared = AtlasClient::prepare(id).expect("prepare");
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let handle = std::thread::spawn(move || {
        let mut svc = AtlasService::new(r, PosixExecutor::new());
        svc.register(id).expect("register");
        svc.run();
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    let client = AtlasClient::connect(prepared).expect("connect");
    (client, running, handle)
}

fn teardown(id: u32, running: Arc<AtomicBool>, handle: std::thread::JoinHandle<()>) {
    running.store(false, Ordering::Relaxed);
    handle.join().expect("service thread panicked");
    cleanup_shm(id);
}

fn bench_read_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_comparison");
    let dir = tempfile::tempdir().unwrap();

    for size in [4096usize, 16384, 65536] {
        group.throughput(Throughput::Bytes(size as u64));
        let path = dir.path().join(format!("read_{size}.dat"));
        let path_str = path.to_str().unwrap();
        std::fs::write(&path, vec![0xABu8; size]).unwrap();

        // Raw executor baseline
        {
            let mut ex = PosixExecutor::new();
            let fd = ex.open(path_str, libc::O_RDONLY, 0) as RawFd;
            let mut buf = vec![0u8; size];
            group.bench_with_input(BenchmarkId::new("raw_executor", size), &size, |b, &sz| {
                b.iter(|| ex.pread(fd, buf.as_mut_ptr(), sz, 0));
            });
            ex.close(fd);
        }

        // Via shared memory
        {
            let id = 7000 + size as u32;
            let (mut client, running, handle) = setup_pair(id);
            let vfd = client.open(path_str, libc::O_RDONLY as u32).expect("open");
            let mut buf = vec![0u8; size];
            group.bench_with_input(BenchmarkId::new("via_shm", size), &size, |b, &sz| {
                b.iter(|| client.read(vfd, &mut buf[..sz], 0).unwrap());
            });
            client.close(vfd).ok();
            teardown(id, running, handle);
        }
    }
    group.finish();
}

fn bench_write_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_comparison");
    let dir = tempfile::tempdir().unwrap();

    for size in [4096usize, 16384, 65536] {
        group.throughput(Throughput::Bytes(size as u64));
        let data = vec![0xCDu8; size];
        let path = dir.path().join(format!("write_{size}.dat"));
        let path_str = path.to_str().unwrap();

        {
            let mut ex = PosixExecutor::new();
            let fd = ex.open(path_str, libc::O_CREAT | libc::O_RDWR, 0o644) as RawFd;
            group.bench_with_input(BenchmarkId::new("raw_executor", size), &size, |b, &sz| {
                b.iter(|| ex.pwrite(fd, data.as_ptr(), sz, 0));
            });
            ex.close(fd);
        }

        {
            let id = 7100 + size as u32;
            let (mut client, running, handle) = setup_pair(id);
            let vfd = client.open(path_str, (libc::O_CREAT | libc::O_RDWR) as u32).expect("open");
            group.bench_with_input(BenchmarkId::new("via_shm", size), &size, |b, &sz| {
                b.iter(|| client.write(vfd, &data[..sz], 0).unwrap());
            });
            client.close(vfd).ok();
            teardown(id, running, handle);
        }
    }
    group.finish();
}

fn bench_fsync_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("fsync_comparison");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fsync_cmp.dat");
    let path_str = path.to_str().unwrap();
    std::fs::write(&path, vec![0u8; 4096]).unwrap();

    {
        let mut ex = PosixExecutor::new();
        let fd = ex.open(path_str, libc::O_RDWR, 0) as RawFd;
        group.bench_function("raw_executor", |b| b.iter(|| ex.fsync(fd)));
        ex.close(fd);
    }

    {
        let id = 7200;
        let (mut client, running, handle) = setup_pair(id);
        let vfd = client.open(path_str, libc::O_RDWR as u32).expect("open");
        group.bench_function("via_shm", |b| b.iter(|| client.sync(vfd).unwrap()));
        client.close(vfd).ok();
        teardown(id, running, handle);
    }

    group.finish();
}

criterion_group!(benches, bench_read_comparison, bench_write_comparison, bench_fsync_comparison);
criterion_main!(benches);
