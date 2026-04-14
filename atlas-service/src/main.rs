use atlas_service::executor::PosixExecutor;
use atlas_service::service::AtlasService;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

static STOP: AtomicBool = AtomicBool::new(false);

fn main() {
    let running = Arc::new(AtomicBool::new(true));

    unsafe {
        libc::signal(libc::SIGINT, sig_handler as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, sig_handler as *const () as libc::sighandler_t);
    }

    let r = running.clone();
    std::thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        r.store(false, Ordering::Relaxed);
    });

    #[cfg(target_os = "linux")]
    {
        let use_uring = std::env::var("ATLAS_USE_URING").is_ok();
        if use_uring {
            let executor = atlas_service::executor::uring::IoUringExecutor::new(256)
                .expect("io_uring init failed");
            let mut svc = AtlasService::new(running, executor);
            register_and_run(&mut svc);
            return;
        }
    }

    let mut svc = AtlasService::new(running, PosixExecutor::new());
    register_and_run(&mut svc);
}

fn register_and_run<E: atlas_service::executor::IoExecutor>(svc: &mut AtlasService<E>) {
    for arg in std::env::args().skip(1) {
        if let Ok(id) = arg.parse::<u32>() {
            match svc.register(id) {
                Ok(()) => eprintln!("Registered instance {id}"),
                Err(e) => eprintln!("Failed to register instance {id}: {e}"),
            }
        }
    }
    eprintln!("Atlas I/O Service running");
    svc.run();
    eprintln!("Atlas I/O Service stopped");
}

extern "C" fn sig_handler(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
