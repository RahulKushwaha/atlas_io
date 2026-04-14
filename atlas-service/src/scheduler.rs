use atlas_protocol::IoPriority;
use crate::batch::MergedOp;
use std::collections::VecDeque;

pub struct Scheduler {
    critical: VecDeque<MergedOp>,
    high: VecDeque<MergedOp>,
    low: VecDeque<MergedOp>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            critical: VecDeque::new(),
            high: VecDeque::new(),
            low: VecDeque::new(),
        }
    }

    pub fn enqueue(&mut self, op: MergedOp) {
        match IoPriority::from_u8(op.req.priority) {
            Some(IoPriority::Critical) => self.critical.push_back(op),
            Some(IoPriority::High) => self.high.push_back(op),
            _ => self.low.push_back(op),
        }
    }

    pub fn enqueue_all(&mut self, ops: Vec<MergedOp>) {
        for op in ops { self.enqueue(op); }
    }

    /// Drain up to `max` ops in priority order: Critical > High > Low.
    pub fn drain(&mut self, max: usize) -> Vec<MergedOp> {
        let mut out = Vec::with_capacity(max);
        for q in [&mut self.critical, &mut self.high, &mut self.low] {
            while out.len() < max {
                match q.pop_front() {
                    Some(op) => out.push(op),
                    None => break,
                }
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.critical.is_empty() && self.high.is_empty() && self.low.is_empty()
    }

    pub fn len(&self) -> usize {
        self.critical.len() + self.high.len() + self.low.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_protocol::{IoOp, IoRequest};
    use crate::batch::MergedOp;

    fn make_op(id: u64, priority: IoPriority) -> MergedOp {
        let req = IoRequest::new(id, IoOp::Read, priority);
        MergedOp { req, channel_idx: 0, sources: vec![(id, 0, 4096)] }
    }

    #[test]
    fn priority_ordering() {
        let mut sched = Scheduler::new();
        sched.enqueue(make_op(1, IoPriority::Low));
        sched.enqueue(make_op(2, IoPriority::Critical));
        sched.enqueue(make_op(3, IoPriority::High));

        let ops = sched.drain(10);
        assert_eq!(ops[0].req.id, 2); // Critical first
        assert_eq!(ops[1].req.id, 3); // High second
        assert_eq!(ops[2].req.id, 1); // Low last
    }
}
