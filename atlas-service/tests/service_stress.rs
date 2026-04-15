//! Integration tests that drive AtlasService + AtlasClient end-to-end,
//! exercising the exact pattern used by `benches/e2e_bench.rs`.
//!
//! These tests reproduce the bench's hot path so we can catch hangs or
//! dropped responses without the criterion warm-up masking the symptom.

use atlas_client::AtlasClient;
use atlas_client::channel::cleanup_shm;
use atlas_service::executor::PosixExecutor;
use atlas_service::service::AtlasService;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn start_service(id: u32) -> (AtlasClient, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    cleanup_shm(id);
    let prepared = AtlasClient::prepare(id).expect("prepare");
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let handle = std::thread::spawn(move || {
        let mut svc = AtlasService::new(r, PosixExecutor::new());
        svc.register(id).expect("register");
        svc.run();
    });
    std::thread::sleep(Duration::from_millis(50));
    let client = AtlasClient::connect(prepared).expect("connect");
    (client, running, handle)
}

fn stop_service(id: u32, running: Arc<AtomicBool>, handle: std::thread::JoinHandle<()>) {
    running.store(false, Ordering::Relaxed);
    let _ = handle.join();
    cleanup_shm(id);
}

/// Run `f` on a background thread with a deadline. Panics with a useful
/// message on timeout instead of hanging the test runner forever.
fn run_with_deadline<F>(label: &str, deadline: Duration, f: F)
where
    F: FnOnce() + Send + 'static,
{
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    let t = std::thread::spawn(move || {
        f();
        d.store(true, Ordering::Release);
    });
    let start = Instant::now();
    while start.elapsed() < deadline {
        if done.load(Ordering::Acquire) {
            t.join().expect("worker panicked");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{label}: deadline {:?} exceeded", deadline);
}

#[test]
fn open_then_close_roundtrip() {
    let id = 8000;
    let (mut client, running, handle) = start_service(id);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.dat");
    std::fs::File::create(&p)
        .unwrap()
        .write_all(b"hello")
        .unwrap();

    let vfd = client
        .open(p.to_str().unwrap(), libc::O_RDONLY as u32)
        .expect("open");
    assert!(vfd > 0);
    client.close(vfd).expect("close");
    stop_service(id, running, handle);
}

#[test]
fn single_read_returns_file_contents() {
    let id = 8001;
    let (mut client, running, handle) = start_service(id);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.dat");
    std::fs::write(&p, b"abcdefghij").unwrap();

    let vfd = client
        .open(p.to_str().unwrap(), libc::O_RDONLY as u32)
        .expect("open");
    let mut buf = [0u8; 10];
    let n = client.read(vfd, &mut buf, 0).expect("read");
    assert_eq!(n, 10);
    assert_eq!(&buf, b"abcdefghij");
    client.close(vfd).ok();
    stop_service(id, running, handle);
}

/// The bench does millions of sequential reads during warm-up. This test
/// reproduces that loop at a smaller scale so hangs surface deterministically.
#[test]
fn sequential_reads_stress() {
    let id = 8002;
    let (mut client, running, handle) = start_service(id);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("read.dat");
    let size = 4096usize;
    std::fs::write(&p, vec![0xABu8; size]).unwrap();

    let vfd = client
        .open(p.to_str().unwrap(), libc::O_RDONLY as u32)
        .expect("open");

    run_with_deadline(
        "sequential_reads_stress",
        Duration::from_secs(10),
        move || {
            let mut buf = vec![0u8; size];
            for i in 0..100_000u64 {
                let n = client
                    .read(vfd, &mut buf, 0)
                    .unwrap_or_else(|e| panic!("read #{i} failed with errno {e}"));
                assert_eq!(n, size, "short read at iteration {i}");
                if i.is_power_of_two() {
                    assert!(
                        buf.iter().all(|&b| b == 0xAB),
                        "corrupted buffer at iter {i}"
                    );
                }
            }
            client.close(vfd).ok();
        },
    );

    stop_service(id, running, handle);
}

#[test]
fn sequential_writes_stress() {
    let id = 8003;
    let (mut client, running, handle) = start_service(id);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("write.dat");
    let size = 4096usize;

    let vfd = client
        .open(p.to_str().unwrap(), (libc::O_CREAT | libc::O_RDWR) as u32)
        .expect("open");

    run_with_deadline(
        "sequential_writes_stress",
        Duration::from_secs(10),
        move || {
            let data = vec![0xCDu8; size];
            for i in 0..100_000u64 {
                let n = client
                    .write(vfd, &data, 0)
                    .unwrap_or_else(|e| panic!("write #{i} failed with errno {e}"));
                assert_eq!(n, size, "short write at iteration {i}");
            }
            client.close(vfd).ok();
        },
    );

    stop_service(id, running, handle);
}

#[test]
fn fsync_roundtrip() {
    let id = 8004;
    let (mut client, running, handle) = start_service(id);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.dat");
    std::fs::write(&p, vec![0u8; 4096]).unwrap();

    let vfd = client
        .open(p.to_str().unwrap(), libc::O_RDWR as u32)
        .expect("open");
    client.sync(vfd).expect("sync");
    client.close(vfd).ok();
    stop_service(id, running, handle);
}
