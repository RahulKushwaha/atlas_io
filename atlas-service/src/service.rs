use crate::batch::{BatchEngine, MergedOp};
use crate::channel::ServiceChannel;
use crate::executor::IoExecutor;
use crate::scheduler::Scheduler;
use atlas_protocol::{IoOp, IoRequest, IoResponse};
use std::collections::HashMap;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AtlasService<E: IoExecutor> {
    channels: Vec<ServiceChannel>,
    fd_table: HashMap<u64, RawFd>,
    next_fd: u64,
    running: Arc<AtomicBool>,
    batch: BatchEngine,
    scheduler: Scheduler,
    executor: E,
}

impl<E: IoExecutor> AtlasService<E> {
    pub fn new(running: Arc<AtomicBool>, executor: E) -> Self {
        Self {
            channels: Vec::new(),
            fd_table: HashMap::new(),
            next_fd: 1,
            running,
            batch: BatchEngine::new(),
            scheduler: Scheduler::new(),
            executor,
        }
    }

    pub fn register(&mut self, instance_id: u32) -> std::io::Result<()> {
        let ch = ServiceChannel::join(instance_id)?;
        self.channels.push(ch);
        Ok(())
    }

    pub fn run(&mut self) {
        while self.running.load(Ordering::Relaxed) {
            let mut did_work = false;
            for i in 0..self.channels.len() {
                while let Some(req) = self.channels[i].poll_request() {
                    self.batch.add(req, i);
                    did_work = true;
                }
            }

            if !self.batch.is_empty() {
                let merged = self.batch.drain_merged();
                self.scheduler.enqueue_all(merged);
            }

            if !self.scheduler.is_empty() {
                let ops = self.scheduler.drain(self.scheduler.len());
                for op in ops {
                    self.execute(op);
                }
            }

            if !did_work {
                std::thread::yield_now();
            }
        }
    }

    fn execute(&mut self, op: MergedOp) {
        let ch_idx = op.channel_idx;
        match IoOp::from_u8(op.req.op) {
            Some(IoOp::Open) => {
                let resp = self.handle_open(&op.req);
                self.channels[ch_idx].send_response(&resp);
            }
            Some(IoOp::Read) => self.execute_read(op),
            Some(IoOp::Write) => self.execute_write(op),
            Some(IoOp::Sync) => {
                let resp = self.handle_sync(&op.req);
                self.channels[ch_idx].send_response(&resp);
            }
            Some(IoOp::Close) => {
                let resp = self.handle_close(&op.req);
                self.channels[ch_idx].send_response(&resp);
            }
            None => {
                self.channels[ch_idx].send_response(&IoResponse::err(op.req.id, -1));
            }
        }
    }

    fn execute_read(&mut self, op: MergedOp) {
        let ch_idx = op.channel_idx;
        let Some(&real_fd) = self.fd_table.get(&op.req.fd) else {
            for (id, _, _) in &op.sources {
                self.channels[ch_idx].send_response(&IoResponse::err(*id, -libc::EBADF));
            }
            return;
        };

        let buf = unsafe { self.channels[ch_idx].data.ptr().add(op.req.data_offset as usize) };
        let n = self.executor.pread(real_fd, buf, op.req.len as usize, op.req.offset);

        if n < 0 {
            for (id, _, _) in &op.sources {
                self.channels[ch_idx].send_response(&IoResponse::err(*id, n as i32));
            }
            return;
        }

        for (id, offset_within, orig_len) in &op.sources {
            let mut resp = IoResponse::ok(*id);
            resp.data_offset = op.req.data_offset + offset_within;
            resp.data_len = if (*offset_within as i64) < n {
                ((n as u32) - offset_within).min(*orig_len)
            } else {
                0
            };
            self.channels[ch_idx].send_response(&resp);
        }
    }

    fn execute_write(&mut self, op: MergedOp) {
        let ch_idx = op.channel_idx;
        let Some(&real_fd) = self.fd_table.get(&op.req.fd) else {
            for (id, _, _) in &op.sources {
                self.channels[ch_idx].send_response(&IoResponse::err(*id, -libc::EBADF));
            }
            return;
        };

        let buf = unsafe { self.channels[ch_idx].data.ptr().add(op.req.data_offset as usize) };
        let n = self.executor.pwrite(real_fd, buf, op.req.len as usize, op.req.offset);

        if n < 0 {
            for (id, _, _) in &op.sources {
                self.channels[ch_idx].send_response(&IoResponse::err(*id, n as i32));
            }
            return;
        }

        for (id, _, orig_len) in &op.sources {
            let mut resp = IoResponse::ok(*id);
            resp.data_len = *orig_len;
            self.channels[ch_idx].send_response(&resp);
        }
    }

    fn handle_open(&mut self, req: &IoRequest) -> IoResponse {
        let flags = req.len as i32;
        let rc = self.executor.open(req.path_str(), flags, 0o644);
        if rc < 0 {
            return IoResponse::err(req.id, rc as i32);
        }
        let vfd = self.next_fd;
        self.next_fd += 1;
        self.fd_table.insert(vfd, rc as RawFd);
        let mut resp = IoResponse::ok(req.id);
        resp.fd = vfd;
        resp
    }

    fn handle_sync(&mut self, req: &IoRequest) -> IoResponse {
        let Some(&real_fd) = self.fd_table.get(&req.fd) else {
            return IoResponse::err(req.id, -libc::EBADF);
        };
        let rc = self.executor.fsync(real_fd);
        if rc < 0 { IoResponse::err(req.id, rc as i32) } else { IoResponse::ok(req.id) }
    }

    fn handle_close(&mut self, req: &IoRequest) -> IoResponse {
        let Some(real_fd) = self.fd_table.remove(&req.fd) else {
            return IoResponse::err(req.id, -libc::EBADF);
        };
        let rc = self.executor.close(real_fd);
        if rc < 0 { IoResponse::err(req.id, rc as i32) } else { IoResponse::ok(req.id) }
    }
}
