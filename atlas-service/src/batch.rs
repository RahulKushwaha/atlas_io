use atlas_protocol::{IoOp, IoRequest};

/// A pending request tagged with its channel index.
#[derive(Debug, Clone, Copy)]
pub struct TaggedRequest {
    pub req: IoRequest,
    pub channel_idx: usize,
}

/// A batch of requests that may have been merged.
#[derive(Debug)]
pub struct MergedOp {
    /// The merged request (offset = min offset, len = total span).
    pub req: IoRequest,
    pub channel_idx: usize,
    /// Original requests that were merged into this one.
    pub sources: Vec<(u64, u32, u32)>, // (request_id, offset_within_merged, original_len)
}

pub struct BatchEngine {
    pending: Vec<TaggedRequest>,
}

impl BatchEngine {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(256),
        }
    }

    pub fn add(&mut self, req: IoRequest, channel_idx: usize) {
        self.pending.push(TaggedRequest { req, channel_idx });
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Drain pending requests, merging adjacent reads/writes on the same fd.
    /// Non-mergeable ops (Open, Close, Sync) pass through as-is.
    pub fn drain_merged(&mut self) -> Vec<MergedOp> {
        if self.pending.is_empty() {
            return Vec::new();
        }

        let mut result = Vec::with_capacity(self.pending.len());

        // Separate mergeable (Read/Write) from non-mergeable
        let mut reads: Vec<TaggedRequest> = Vec::new();
        let mut writes: Vec<TaggedRequest> = Vec::new();
        let mut others: Vec<TaggedRequest> = Vec::new();

        for tr in self.pending.drain(..) {
            match IoOp::from_u8(tr.req.op) {
                Some(IoOp::Read) => reads.push(tr),
                Some(IoOp::Write) => writes.push(tr),
                _ => others.push(tr),
            }
        }

        // Pass-through non-mergeable ops
        for tr in others {
            result.push(MergedOp {
                req: tr.req,
                channel_idx: tr.channel_idx,
                sources: vec![(tr.req.id, 0, tr.req.len)],
            });
        }

        // Merge adjacent reads by (channel_idx, fd)
        merge_adjacent(&mut reads, &mut result);
        merge_adjacent(&mut writes, &mut result);

        result
    }
}

fn merge_adjacent(ops: &mut Vec<TaggedRequest>, result: &mut Vec<MergedOp>) {
    if ops.is_empty() {
        return;
    }

    // Sort by (channel_idx, fd, offset)
    ops.sort_by(|a, b| {
        a.channel_idx
            .cmp(&b.channel_idx)
            .then(a.req.fd.cmp(&b.req.fd))
            .then(a.req.offset.cmp(&b.req.offset))
    });

    let mut i = 0;
    while i < ops.len() {
        let first = &ops[i];
        let mut merged_req = first.req;
        let ch_idx = first.channel_idx;
        let mut end = first.req.offset + first.req.len as u64;
        let mut sources = vec![(first.req.id, 0u32, first.req.len)];

        let mut j = i + 1;
        while j < ops.len() {
            let next = &ops[j];
            // Same channel and fd, and adjacent or overlapping
            if next.channel_idx == ch_idx && next.req.fd == merged_req.fd && next.req.offset <= end
            {
                let offset_within = (next.req.offset - merged_req.offset) as u32;
                sources.push((next.req.id, offset_within, next.req.len));
                let next_end = next.req.offset + next.req.len as u64;
                if next_end > end {
                    end = next_end;
                }
                j += 1;
            } else {
                break;
            }
        }

        merged_req.len = (end - merged_req.offset) as u32;
        result.push(MergedOp {
            req: merged_req,
            channel_idx: ch_idx,
            sources,
        });
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_protocol::IoPriority;

    #[test]
    fn merge_adjacent_reads() {
        let mut engine = BatchEngine::new();

        let mut r1 = IoRequest::new(1, IoOp::Read, IoPriority::High);
        r1.fd = 10;
        r1.offset = 0;
        r1.len = 4096;
        r1.data_offset = 0;
        engine.add(r1, 0);

        let mut r2 = IoRequest::new(2, IoOp::Read, IoPriority::High);
        r2.fd = 10;
        r2.offset = 4096;
        r2.len = 4096;
        r2.data_offset = 4096;
        engine.add(r2, 0);

        let merged = engine.drain_merged();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].req.len, 8192);
        assert_eq!(merged[0].sources.len(), 2);
    }

    #[test]
    fn no_merge_different_fds() {
        let mut engine = BatchEngine::new();

        let mut r1 = IoRequest::new(1, IoOp::Read, IoPriority::High);
        r1.fd = 10;
        r1.offset = 0;
        r1.len = 4096;
        engine.add(r1, 0);

        let mut r2 = IoRequest::new(2, IoOp::Read, IoPriority::High);
        r2.fd = 11;
        r2.offset = 0;
        r2.len = 4096;
        engine.add(r2, 0);

        let merged = engine.drain_merged();
        assert_eq!(merged.len(), 2);
    }
}
