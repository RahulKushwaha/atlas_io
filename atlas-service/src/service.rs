use crate::batch::MergedOp;
use crate::channel::ServiceChannel;
use crate::executor::{BatchKind, BatchOp, IoExecutor};
use crate::pipeline::{PassThrough, Pipeline};
use atlas_protocol::{IoOp, IoRequest, IoResponse};
use std::collections::HashMap;
use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct AtlasService<E: IoExecutor, P: Pipeline = PassThrough> {
    channels: Vec<ServiceChannel>,
    fd_table: HashMap<u64, RawFd>,
    next_fd: u64,
    running: Arc<AtomicBool>,
    pipeline: P,
    executor: E,
}

impl<E: IoExecutor> AtlasService<E, PassThrough> {
    pub fn new(running: Arc<AtomicBool>, executor: E) -> Self {
        Self::with_pipeline(running, executor, PassThrough::new())
    }
}

impl<E: IoExecutor, P: Pipeline> AtlasService<E, P> {
    pub fn with_pipeline(running: Arc<AtomicBool>, executor: E, pipeline: P) -> Self {
        Self {
            channels: Vec::new(),
            fd_table: HashMap::new(),
            next_fd: 1,
            running,
            pipeline,
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
                    self.pipeline.add(req, i);
                    did_work = true;
                }
            }

            if !self.pipeline.is_empty() {
                let ops = self.pipeline.drain();
                self.execute_round(ops);
            }

            if !did_work {
                std::thread::yield_now();
            }
        }
    }

    /// Handle one drained round: structural ops (open/close) run inline
    /// because they mutate `fd_table`; data-plane ops (read/write/fsync)
    /// are collected into a single batch so executors that support it
    /// (io_uring) can submit the whole round in one syscall.
    fn execute_round(&mut self, ops: Vec<MergedOp>) {
        let mut batch: Vec<BatchOp> = Vec::with_capacity(ops.len());
        // Index in `ops` for each corresponding `batch` entry — lets us
        // recover the source fan-out when building responses.
        let mut batch_origin: Vec<usize> = Vec::with_capacity(ops.len());

        for (i, op) in ops.iter().enumerate() {
            match IoOp::from_u8(op.req.op) {
                Some(IoOp::Open) => {
                    let resp = self.handle_open(&op.req);
                    self.channels[op.channel_idx].send_response(&resp);
                }
                Some(IoOp::Close) => {
                    let resp = self.handle_close(&op.req);
                    self.channels[op.channel_idx].send_response(&resp);
                }
                Some(IoOp::Read) => {
                    if let Some(bop) = self.prepare_read(op) {
                        batch.push(bop);
                        batch_origin.push(i);
                    }
                }
                Some(IoOp::Write) => {
                    if let Some(bop) = self.prepare_write(op) {
                        batch.push(bop);
                        batch_origin.push(i);
                    }
                }
                Some(IoOp::Sync) => {
                    if let Some(bop) = self.prepare_fsync(op) {
                        batch.push(bop);
                        batch_origin.push(i);
                    }
                }
                None => {
                    self.channels[op.channel_idx].send_response(&IoResponse::err(op.req.id, -1));
                }
            }
        }

        if batch.is_empty() {
            return;
        }

        self.executor.execute_batch(&mut batch);

        for (bop, &orig_idx) in batch.iter().zip(batch_origin.iter()) {
            let op = &ops[orig_idx];
            match bop.kind {
                BatchKind::Pread { .. } => self.send_read_responses(op, bop.result),
                BatchKind::Pwrite { .. } => self.send_write_responses(op, bop.result),
                BatchKind::Fsync { .. } => {
                    let resp = if bop.result < 0 {
                        IoResponse::err(op.req.id, bop.result as i32)
                    } else {
                        IoResponse::ok(op.req.id)
                    };
                    self.channels[op.channel_idx].send_response(&resp);
                }
            }
        }
    }

    fn prepare_read(&mut self, op: &MergedOp) -> Option<BatchOp> {
        let Some(&real_fd) = self.fd_table.get(&op.req.fd) else {
            for (id, _, _) in &op.sources {
                self.channels[op.channel_idx].send_response(&IoResponse::err(*id, -libc::EBADF));
            }
            return None;
        };
        let buf = unsafe {
            self.channels[op.channel_idx]
                .data
                .ptr()
                .add(op.req.data_offset as usize)
        };
        Some(BatchOp::new(BatchKind::Pread {
            fd: real_fd,
            buf,
            len: op.req.len as usize,
            offset: op.req.offset,
        }))
    }

    fn prepare_write(&mut self, op: &MergedOp) -> Option<BatchOp> {
        let Some(&real_fd) = self.fd_table.get(&op.req.fd) else {
            for (id, _, _) in &op.sources {
                self.channels[op.channel_idx].send_response(&IoResponse::err(*id, -libc::EBADF));
            }
            return None;
        };
        let buf = unsafe {
            self.channels[op.channel_idx]
                .data
                .ptr()
                .add(op.req.data_offset as usize)
        };
        Some(BatchOp::new(BatchKind::Pwrite {
            fd: real_fd,
            buf,
            len: op.req.len as usize,
            offset: op.req.offset,
        }))
    }

    fn prepare_fsync(&mut self, op: &MergedOp) -> Option<BatchOp> {
        let Some(&real_fd) = self.fd_table.get(&op.req.fd) else {
            self.channels[op.channel_idx].send_response(&IoResponse::err(op.req.id, -libc::EBADF));
            return None;
        };
        Some(BatchOp::new(BatchKind::Fsync { fd: real_fd }))
    }

    fn send_read_responses(&mut self, op: &MergedOp, n: i64) {
        let ch_idx = op.channel_idx;
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

    fn send_write_responses(&mut self, op: &MergedOp, n: i64) {
        let ch_idx = op.channel_idx;
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

    fn handle_close(&mut self, req: &IoRequest) -> IoResponse {
        let Some(real_fd) = self.fd_table.remove(&req.fd) else {
            return IoResponse::err(req.id, -libc::EBADF);
        };
        let rc = self.executor.close(real_fd);
        if rc < 0 {
            IoResponse::err(req.id, rc as i32)
        } else {
            IoResponse::ok(req.id)
        }
    }
}
