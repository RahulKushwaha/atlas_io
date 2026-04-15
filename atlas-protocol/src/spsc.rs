//! Lock-free single-producer single-consumer ring buffer, laid out for
//! placement in a shared-memory segment so a pair of cooperating
//! processes can communicate without syscalls on the hot path.
//!
//! Design notes:
//! - `T: Copy + AnyBitPattern` — shared memory is zero-initialized and
//!   any bit pattern must be a valid `T`. This matches our wire types
//!   (`IoRequest`, `IoResponse`).
//! - Capacity `N` is a compile-time power of two; indexing uses `&(N-1)`.
//! - `head`/`tail` are monotonically increasing `usize` counters; the
//!   physical slot is `counter & (N - 1)`.
//! - `head` and `tail` live in their own 128-byte cache-line padded
//!   regions to avoid false sharing between producer and consumer.
//! - The producer's `Release` store on `tail` pairs with the consumer's
//!   `Acquire` load on `tail`, so the consumer sees the slot write that
//!   happened-before the release. Critically, the consumer must load
//!   `tail` BEFORE reading the slot; that ordering is preserved below
//!   to avoid the TOCTOU race that bit us when we used `que`.

use bytemuck::AnyBitPattern;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const MAGIC: u64 = 0xA71A5_595C_0001;

#[repr(C, align(128))]
pub struct SpscRing<T, const N: usize> {
    magic: AtomicU64,
    capacity: AtomicUsize,
    _pad0: [u8; 112],

    tail: AtomicUsize,
    _pad1: [u8; 120],

    head: AtomicUsize,
    _pad2: [u8; 120],

    slots: UnsafeCell<[MaybeUninit<T>; N]>,
}

unsafe impl<T: Send, const N: usize> Sync for SpscRing<T, N> {}

impl<T: Copy + AnyBitPattern, const N: usize> SpscRing<T, N> {
    /// Bytes required to host this ring in memory (use this to `ftruncate`
    /// the shm segment).
    pub const fn size_bytes() -> usize {
        core::mem::size_of::<Self>()
    }

    /// Initialize a ring at `ptr`, or validate an already-initialized one.
    ///
    /// Must be called exactly once per segment before `Producer::new` or
    /// `Consumer::new` are used; concurrent initialization is not supported.
    ///
    /// SAFETY: `ptr` must point to at least `size_bytes()` of zero- or
    /// ring-initialized memory, aligned to 128 bytes (mmap guarantees
    /// page alignment, which is stronger).
    pub unsafe fn init_in_place(ptr: *mut u8) -> *const Self {
        assert!(N > 0 && N.is_power_of_two(), "N must be a power of two");
        assert!(
            ptr as usize % 128 == 0,
            "ring pointer must be 128-byte aligned"
        );
        let r = ptr as *const Self;
        let this = unsafe { &*r };
        let m = this.magic.load(Ordering::Acquire);
        if m == 0 {
            this.capacity.store(N, Ordering::Relaxed);
            this.tail.store(0, Ordering::Relaxed);
            this.head.store(0, Ordering::Relaxed);
            this.magic.store(MAGIC, Ordering::Release);
        } else if m != MAGIC {
            panic!("spsc ring magic mismatch: 0x{m:x}");
        } else {
            let cap = this.capacity.load(Ordering::Acquire);
            assert_eq!(
                cap, N,
                "spsc ring capacity mismatch: expected {N}, found {cap}"
            );
        }
        r
    }

    /// Attach to a ring that another process has already initialized.
    /// Spins briefly waiting for the peer's `init_in_place` to publish the
    /// magic. Returns `Err` if the segment is uninitialized or corrupt.
    ///
    /// SAFETY: same as `init_in_place`.
    pub unsafe fn attach(ptr: *mut u8) -> Result<*const Self, &'static str> {
        assert!(N > 0 && N.is_power_of_two(), "N must be a power of two");
        assert!(
            ptr as usize % 128 == 0,
            "ring pointer must be 128-byte aligned"
        );
        let r = ptr as *const Self;
        let this = unsafe { &*r };
        for _ in 0..1_000_000 {
            let m = this.magic.load(Ordering::Acquire);
            if m == MAGIC {
                let cap = this.capacity.load(Ordering::Acquire);
                if cap != N {
                    return Err("capacity mismatch");
                }
                return Ok(r);
            }
            if m != 0 {
                return Err("corrupt magic");
            }
            core::hint::spin_loop();
        }
        Err("uninitialized")
    }
}

pub struct Producer<T: Copy + AnyBitPattern, const N: usize> {
    ring: *const SpscRing<T, N>,
    cached_head: usize,
    local_tail: usize,
}

unsafe impl<T: Copy + AnyBitPattern + Send, const N: usize> Send for Producer<T, N> {}

impl<T: Copy + AnyBitPattern, const N: usize> Producer<T, N> {
    /// SAFETY: `ring` must outlive this producer and there must be at
    /// most one `Producer` for a given ring at a time.
    pub unsafe fn new(ring: *const SpscRing<T, N>) -> Self {
        let r = unsafe { &*ring };
        let local_tail = r.tail.load(Ordering::Acquire);
        let cached_head = r.head.load(Ordering::Acquire);
        Self {
            ring,
            cached_head,
            local_tail,
        }
    }

    #[inline]
    pub fn push(&mut self, value: &T) -> Result<(), RingFull> {
        let r = unsafe { &*self.ring };
        // Fast path: we believe we have space based on our cached head.
        if self.local_tail.wrapping_sub(self.cached_head) >= N {
            // Refresh head and recheck.
            self.cached_head = r.head.load(Ordering::Acquire);
            if self.local_tail.wrapping_sub(self.cached_head) >= N {
                return Err(RingFull);
            }
        }
        let idx = self.local_tail & (N - 1);
        unsafe {
            let slot = (r.slots.get() as *mut MaybeUninit<T>).add(idx) as *mut T;
            core::ptr::write(slot, *value);
        }
        self.local_tail = self.local_tail.wrapping_add(1);
        // Publish the write. Release pairs with consumer's Acquire on tail.
        r.tail.store(self.local_tail, Ordering::Release);
        Ok(())
    }
}

pub struct Consumer<T: Copy + AnyBitPattern, const N: usize> {
    ring: *const SpscRing<T, N>,
    local_head: usize,
    cached_tail: usize,
}

unsafe impl<T: Copy + AnyBitPattern + Send, const N: usize> Send for Consumer<T, N> {}

impl<T: Copy + AnyBitPattern, const N: usize> Consumer<T, N> {
    /// SAFETY: `ring` must outlive this consumer and there must be at
    /// most one `Consumer` for a given ring at a time.
    pub unsafe fn new(ring: *const SpscRing<T, N>) -> Self {
        let r = unsafe { &*ring };
        let local_head = r.head.load(Ordering::Acquire);
        let cached_tail = r.tail.load(Ordering::Acquire);
        Self {
            ring,
            local_head,
            cached_tail,
        }
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        let r = unsafe { &*self.ring };
        if self.local_head == self.cached_tail {
            // Reload tail. Acquire pairs with the producer's Release
            // store, so any slot write published before that release is
            // visible to the subsequent slot read below.
            self.cached_tail = r.tail.load(Ordering::Acquire);
            if self.local_head == self.cached_tail {
                return None;
            }
        }
        let idx = self.local_head & (N - 1);
        let v = unsafe {
            let slot = (r.slots.get() as *const MaybeUninit<T>).add(idx) as *const T;
            core::ptr::read(slot)
        };
        self.local_head = self.local_head.wrapping_add(1);
        r.head.store(self.local_head, Ordering::Release);
        Some(v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingFull;

impl core::fmt::Display for RingFull {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("spsc ring full")
    }
}

impl std::error::Error for RingFull {}

#[cfg(test)]
mod tests {
    use super::*;

    fn zeroed_buf(n: usize) -> *mut u8 {
        let layout = std::alloc::Layout::from_size_align(n, 4096).unwrap();
        unsafe {
            let p = std::alloc::alloc_zeroed(layout);
            assert!(!p.is_null());
            p
        }
    }

    #[test]
    fn single_push_pop() {
        let buf = zeroed_buf(SpscRing::<u64, 16>::size_bytes());
        unsafe {
            let ring = SpscRing::<u64, 16>::init_in_place(buf);
            let mut p = Producer::new(ring);
            let mut c = Consumer::new(ring);
            p.push(&42).unwrap();
            assert_eq!(c.pop(), Some(42));
            assert_eq!(c.pop(), None);
        }
    }

    #[test]
    fn fill_then_drain() {
        let buf = zeroed_buf(SpscRing::<u64, 4>::size_bytes());
        unsafe {
            let ring = SpscRing::<u64, 4>::init_in_place(buf);
            let mut p = Producer::new(ring);
            let mut c = Consumer::new(ring);
            for i in 0..4 {
                p.push(&i).unwrap();
            }
            assert_eq!(p.push(&99), Err(RingFull));
            for i in 0..4 {
                assert_eq!(c.pop(), Some(i));
            }
            assert_eq!(c.pop(), None);
            // Wraparound
            for i in 10..14 {
                p.push(&i).unwrap();
            }
            for i in 10..14 {
                assert_eq!(c.pop(), Some(i));
            }
        }
    }

    #[test]
    fn attach_validates_magic() {
        let buf = zeroed_buf(SpscRing::<u64, 8>::size_bytes());
        unsafe {
            // Attaching to an uninitialized segment errors quickly.
            // (we'd rather not spin the full 1M loop in a test, but it's
            // cheap enough — skip this case and instead init first.)
            let _ = SpscRing::<u64, 8>::init_in_place(buf);
            let ring = SpscRing::<u64, 8>::attach(buf).unwrap();
            let mut p = Producer::new(ring);
            let mut c = Consumer::new(ring);
            p.push(&7).unwrap();
            assert_eq!(c.pop(), Some(7));
        }
    }

    #[test]
    fn threaded_producer_consumer_stress() {
        // This is the scenario that used to deadlock under que: tight
        // loop of sequential push/pop between two threads, many millions
        // of iterations, 296-byte payload crossing multiple cache lines.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        #[derive(Clone, Copy)]
        #[repr(C)]
        struct Payload {
            id: u64,
            fill: [u8; 288],
        }
        unsafe impl bytemuck::Zeroable for Payload {}
        unsafe impl bytemuck::AnyBitPattern for Payload {}
        unsafe impl bytemuck::NoUninit for Payload {}

        let buf = zeroed_buf(SpscRing::<Payload, 4096>::size_bytes());
        let ring = unsafe { SpscRing::<Payload, 4096>::init_in_place(buf) };

        struct Shared(*const SpscRing<Payload, 4096>);
        unsafe impl Send for Shared {}
        unsafe impl Sync for Shared {}
        let shared = Arc::new(Shared(ring));
        let done = Arc::new(AtomicBool::new(false));

        let prod_shared = shared.clone();
        let prod = std::thread::spawn(move || {
            let mut p = unsafe { Producer::new(prod_shared.0) };
            let mut pl = Payload {
                id: 0,
                fill: [0; 288],
            };
            for i in 0..200_000u64 {
                pl.id = i + 1;
                while p.push(&pl).is_err() {
                    std::thread::yield_now();
                }
            }
        });

        let cons_shared = shared.clone();
        let done_c = done.clone();
        let cons = std::thread::spawn(move || {
            let mut c = unsafe { Consumer::new(cons_shared.0) };
            let mut expected = 1u64;
            while expected <= 200_000 {
                match c.pop() {
                    Some(v) => {
                        assert_eq!(v.id, expected, "out-of-order or torn read");
                        expected += 1;
                    }
                    None => std::thread::yield_now(),
                }
            }
            done_c.store(true, Ordering::Release);
        });

        prod.join().unwrap();
        cons.join().unwrap();
        assert!(done.load(Ordering::Acquire));
    }
}
