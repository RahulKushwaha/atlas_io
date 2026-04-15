//! Concurrent-client bench: shows the io_uring batching win.
//!
//! The per-op e2e bench uses a single sequential client, so each
//! `pipeline.drain()` has exactly one op and `execute_batch` has nothing
//! to amortise. Here we pre-spawn N worker threads — one per registered
//! instance — that each fire a single `read` per round. The service
//! polls all channels in one loop iteration before draining, so the
//! pipeline hands a batch of up to N ops to `execute_batch`.
//!
//! That's the regime where io_uring's batched submit (one syscall per N
//! ops) should pull ahead of pread-per-op.

use atlas_client::AtlasClient;
use atlas_client::channel::cleanup_shm;
#[cfg(target_os = "linux")]
use atlas_service::executor::uring::IoUringExecutor;
use atlas_service::executor::{IoExecutor, PosixExecutor};
use atlas_service::service::AtlasService;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

struct Harness {
    round: Arc<AtomicU64>,
    completed: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
    num_workers: usize,
    svc_running: Arc<AtomicBool>,
    svc_handle: Option<std::thread::JoinHandle<()>>,
    ids: Vec<u32>,
}

impl Harness {
    /// Trigger one round: release all workers, wait for all to complete one op.
    fn tick(&self) {
        self.completed.store(0, Ordering::SeqCst);
        self.round.fetch_add(1, Ordering::SeqCst);
        while self.completed.load(Ordering::Acquire) < self.num_workers {
            std::hint::spin_loop();
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake any worker still waiting on `round`.
        self.round.fetch_add(1, Ordering::SeqCst);
        for h in self.workers.drain(..) {
            let _ = h.join();
        }
        self.svc_running.store(false, Ordering::Relaxed);
        if let Some(h) = self.svc_handle.take() {
            let _ = h.join();
        }
        for id in &self.ids {
            cleanup_shm(*id);
        }
    }
}

fn setup_read_harness<E, F>(
    num_clients: u32,
    base_id: u32,
    make_exec: F,
    path: &str,
    size: usize,
) -> Harness
where
    E: IoExecutor + Send + 'static,
    F: FnOnce() -> E + Send + 'static,
{
    let ids: Vec<u32> = (0..num_clients).map(|i| base_id + i).collect();
    for id in &ids {
        cleanup_shm(*id);
    }
    let prepared: Vec<_> = ids
        .iter()
        .map(|&id| AtlasClient::prepare(id).unwrap())
        .collect();

    let svc_running = Arc::new(AtomicBool::new(true));
    let r = svc_running.clone();
    let ids_for_svc = ids.clone();
    let svc_handle = std::thread::spawn(move || {
        let mut svc = AtlasService::new(r, make_exec());
        for id in &ids_for_svc {
            svc.register(*id).expect("register");
        }
        svc.run();
    });
    // Let the service create its response rings.
    std::thread::sleep(std::time::Duration::from_millis(100));

    let clients: Vec<(AtlasClient, u64)> = prepared
        .into_iter()
        .map(|p| {
            let mut c = AtlasClient::connect(p).expect("connect");
            let vfd = c.open(path, libc::O_RDONLY as u32).expect("open");
            (c, vfd)
        })
        .collect();

    let round = Arc::new(AtomicU64::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let num_workers = clients.len();

    let workers: Vec<_> = clients
        .into_iter()
        .map(|(mut client, vfd)| {
            let round = round.clone();
            let completed = completed.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; size];
                let mut seen = 0u64;
                loop {
                    let r = loop {
                        let v = round.load(Ordering::Acquire);
                        if v != seen {
                            break v;
                        }
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::hint::spin_loop();
                    };
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    client.read(vfd, &mut buf, 0).expect("read");
                    completed.fetch_add(1, Ordering::Release);
                    seen = r;
                }
            })
        })
        .collect();

    Harness {
        round,
        completed,
        stop,
        workers,
        num_workers,
        svc_running,
        svc_handle: Some(svc_handle),
        ids,
    }
}

fn bench_concurrent_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_reads_4k");
    let dir = tempfile::tempdir().unwrap();
    let size = 4096usize;
    let path = dir.path().join("data.dat");
    std::fs::write(&path, vec![0xABu8; size]).unwrap();
    let path_str = path.to_str().unwrap().to_string();

    for num_clients in [1u32, 4, 8, 16, 32] {
        group.throughput(Throughput::Elements(num_clients as u64));

        {
            let h = setup_read_harness(
                num_clients,
                10_000 + num_clients * 200,
                PosixExecutor::new,
                &path_str,
                size,
            );
            group.bench_with_input(
                BenchmarkId::new("posix", num_clients),
                &num_clients,
                |b, _| b.iter(|| h.tick()),
            );
        }

        #[cfg(target_os = "linux")]
        {
            let h = setup_read_harness(
                num_clients,
                20_000 + num_clients * 200,
                || IoUringExecutor::new(512).expect("uring"),
                &path_str,
                size,
            );
            group.bench_with_input(
                BenchmarkId::new("uring", num_clients),
                &num_clients,
                |b, _| b.iter(|| h.tick()),
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_concurrent_reads);
criterion_main!(benches);
