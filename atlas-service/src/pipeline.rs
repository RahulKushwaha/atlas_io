//! Pluggable request pipeline between `poll_request` and `execute`.
//!
//! The service loop hands each incoming `IoRequest` to a `Pipeline`, then
//! drains `MergedOp`s back out for execution. Implementations are free to
//! reorder, merge, or delay requests — or pass them through unchanged.
//!
//! Two stages ship today:
//!
//! - [`PassThrough`] (the default): FIFO, no merging, no prioritisation.
//!   Cheapest per-op cost, best for a single-client workload where the
//!   batching/prio machinery has nothing to optimise.
//! - [`Batched`]: coalesces adjacent reads/writes via [`BatchEngine`] and
//!   drains in priority order via [`Scheduler`]. Enable when many
//!   RocksDB instances share one service and issue overlapping I/O.
//!
//! Pipelines are stackable: a custom stage can wrap another `Pipeline`
//! to add behaviour (rate limiting, tracing, fairness) without touching
//! the service loop.

use crate::batch::{BatchEngine, MergedOp, TaggedRequest};
use crate::scheduler::Scheduler;
use atlas_protocol::IoRequest;
use std::collections::VecDeque;

pub trait Pipeline {
    fn add(&mut self, req: IoRequest, channel_idx: usize);
    fn is_empty(&self) -> bool;
    /// Remove all ready ops for execution.
    fn drain(&mut self) -> Vec<MergedOp>;
}

/// FIFO passthrough with no batching or prioritisation. Each request
/// becomes a single-source `MergedOp`.
pub struct PassThrough {
    pending: VecDeque<TaggedRequest>,
}

impl PassThrough {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::with_capacity(256),
        }
    }
}

impl Default for PassThrough {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline for PassThrough {
    fn add(&mut self, req: IoRequest, channel_idx: usize) {
        self.pending.push_back(TaggedRequest { req, channel_idx });
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn drain(&mut self) -> Vec<MergedOp> {
        let mut out = Vec::with_capacity(self.pending.len());
        for tr in self.pending.drain(..) {
            out.push(MergedOp {
                req: tr.req,
                channel_idx: tr.channel_idx,
                sources: vec![(tr.req.id, 0, tr.req.len)],
            });
        }
        out
    }
}

/// Coalesce adjacent I/Os (via `BatchEngine`) and drain in priority
/// order (via `Scheduler`). Preserves the previous service behaviour
/// for callers that opt in.
pub struct Batched {
    batch: BatchEngine,
    scheduler: Scheduler,
}

impl Batched {
    pub fn new() -> Self {
        Self {
            batch: BatchEngine::new(),
            scheduler: Scheduler::new(),
        }
    }
}

impl Default for Batched {
    fn default() -> Self {
        Self::new()
    }
}

impl Pipeline for Batched {
    fn add(&mut self, req: IoRequest, channel_idx: usize) {
        self.batch.add(req, channel_idx);
    }

    fn is_empty(&self) -> bool {
        self.batch.is_empty() && self.scheduler.is_empty()
    }

    fn drain(&mut self) -> Vec<MergedOp> {
        if !self.batch.is_empty() {
            let merged = self.batch.drain_merged();
            self.scheduler.enqueue_all(merged);
        }
        let n = self.scheduler.len();
        self.scheduler.drain(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_protocol::{IoOp, IoPriority};

    fn req(id: u64, op: IoOp, fd: u64, offset: u64, len: u32) -> IoRequest {
        let mut r = IoRequest::new(id, op, IoPriority::High);
        r.fd = fd;
        r.offset = offset;
        r.len = len;
        r
    }

    #[test]
    fn passthrough_preserves_order_and_does_not_merge() {
        let mut p = PassThrough::new();
        // Two adjacent reads on the same fd that Batched would merge.
        p.add(req(1, IoOp::Read, 10, 0, 4096), 0);
        p.add(req(2, IoOp::Read, 10, 4096, 4096), 0);
        let ops = p.drain();
        assert_eq!(ops.len(), 2, "passthrough must not coalesce");
        assert_eq!(ops[0].req.id, 1);
        assert_eq!(ops[1].req.id, 2);
        assert_eq!(ops[0].sources.len(), 1);
    }

    #[test]
    fn batched_coalesces_adjacent_reads() {
        let mut p = Batched::new();
        p.add(req(1, IoOp::Read, 10, 0, 4096), 0);
        p.add(req(2, IoOp::Read, 10, 4096, 4096), 0);
        let ops = p.drain();
        assert_eq!(ops.len(), 1, "batched must coalesce");
        assert_eq!(ops[0].sources.len(), 2);
        assert_eq!(ops[0].req.len, 8192);
    }

    #[test]
    fn is_empty_tracks_state() {
        let mut p = PassThrough::new();
        assert!(p.is_empty());
        p.add(req(1, IoOp::Read, 10, 0, 4096), 0);
        assert!(!p.is_empty());
        p.drain();
        assert!(p.is_empty());
    }
}
