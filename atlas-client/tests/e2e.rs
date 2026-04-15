use atlas_client::AtlasClient;
use atlas_client::channel::cleanup_shm;
use atlas_service::executor::PosixExecutor;
use atlas_service::service::AtlasService;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Proper startup: prepare clients → start service → connect clients.
fn setup(ids: &[u32]) -> (Vec<AtlasClient>, Arc<AtomicBool>) {
    for &id in ids {
        cleanup_shm(id);
    }

    // Phase 1: prepare all clients (creates req rings + data regions)
    let prepared: Vec<_> = ids
        .iter()
        .map(|&id| AtlasClient::prepare(id).expect("prepare"))
        .collect();

    // Phase 2: start service (joins req rings, creates resp rings)
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let ids_vec = ids.to_vec();
    std::thread::spawn(move || {
        let mut svc = AtlasService::new(r, PosixExecutor::new());
        for id in &ids_vec {
            svc.register(*id).expect("register");
        }
        svc.run();
    });
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Phase 3: connect clients (joins resp rings)
    let clients = prepared
        .into_iter()
        .map(|p| AtlasClient::connect(p).expect("connect"))
        .collect();

    (clients, running)
}

fn teardown(ids: &[u32], running: Arc<AtomicBool>) {
    running.store(false, Ordering::Relaxed);
    std::thread::sleep(std::time::Duration::from_millis(50));
    for &id in ids {
        cleanup_shm(id);
    }
}

#[test]
fn single_instance_write_read() {
    let ids = [9000];
    let (mut clients, running) = setup(&ids);
    let client = &mut clients[0];

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.dat");
    let path_str = path.to_str().unwrap();

    let fd = client
        .open(
            path_str,
            (libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC) as u32,
        )
        .expect("open write");
    let data = b"Hello, Atlas I/O Service!";
    assert_eq!(client.write(fd, data, 0).unwrap(), data.len());
    client.sync(fd).unwrap();
    client.close(fd).unwrap();

    let fd = client
        .open(path_str, libc::O_RDONLY as u32)
        .expect("open read");
    let mut buf = vec![0u8; 64];
    let n = client.read(fd, &mut buf, 0).unwrap();
    assert_eq!(&buf[..n], data);
    client.close(fd).unwrap();

    teardown(&ids, running);
}

#[test]
fn multi_instance_concurrent() {
    let ids: Vec<u32> = (9100..9104).collect();
    let (clients, running) = setup(&ids);
    let dir = tempfile::tempdir().unwrap();

    let handles: Vec<_> = clients
        .into_iter()
        .enumerate()
        .map(|(i, mut client)| {
            let path = dir.path().join(format!("instance_{i}.dat"));
            let r = running.clone();
            std::thread::spawn(move || {
                let path_str = path.to_str().unwrap();
                let payload = format!("data from instance {i}");
                let data = payload.as_bytes();

                let fd = client
                    .open(
                        path_str,
                        (libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC) as u32,
                    )
                    .unwrap();
                client.write(fd, data, 0).unwrap();
                client.sync(fd).unwrap();
                client.close(fd).unwrap();

                let fd = client.open(path_str, libc::O_RDONLY as u32).unwrap();
                let mut buf = vec![0u8; 128];
                let n = client.read(fd, &mut buf, 0).unwrap();
                assert_eq!(&buf[..n], data, "instance {i} mismatch");
                client.close(fd).unwrap();
                let _ = r;
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
    teardown(&ids, running);
}
