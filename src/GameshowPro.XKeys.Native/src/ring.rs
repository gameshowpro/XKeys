// Copyright (c) Barjonas LLC 2026.
//! A bounded, lock-free, single-producer single-consumer ring (the same design as `tgstamp`'s, generic over the entry type).

use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The smallest ring [`Ring::new`] makes, in entries.
pub const MIN_CAPACITY: usize = 16;

/// The largest ring [`Ring::new`] makes, in entries (384 KiB of report entries).
pub const MAX_CAPACITY: usize = 4096;

/// The capacity used when the caller asks for 0.
pub const DEFAULT_CAPACITY: usize = 256;

/// The capacity a request for `requested` entries gets: 0 means [`DEFAULT_CAPACITY`]; anything else is clamped to
/// [`MIN_CAPACITY`]..=[`MAX_CAPACITY`] and rounded up to a power of two.
pub fn capacity_for(requested: usize) -> usize {
    if requested == 0 { DEFAULT_CAPACITY } else { requested.clamp(MIN_CAPACITY, MAX_CAPACITY).next_power_of_two() }
}

/// Keeps the producer's and the consumer's indexes on separate cache lines.
#[repr(C, align(64))]
struct CachePadded<T>(T);

/// A bounded single-producer, single-consumer ring. Lock-free and allocation-free after construction.
///
/// `head` and `tail` count entries ever written and read, and wrap at `usize::MAX`; a slot is `index & mask`.
pub struct Ring<T: Copy> {
    /// The next index the producer writes. Written only by the producer; published with Release.
    head: CachePadded<AtomicUsize>,
    /// The next index the consumer reads. Written only by the consumer; published with Release.
    tail: CachePadded<AtomicUsize>,
    /// Entries the producer could not write because the ring was full.
    dropped: CachePadded<AtomicU64>,
    mask: usize,
    slots: Box<[UnsafeCell<T>]>,
}

// SAFETY: the slots are only touched under the single-producer, single-consumer protocol in `push` and `read_into`, whose
// callers promise that contract; `head` and `tail` order every slot access between the two threads.
unsafe impl<T: Copy + Send> Sync for Ring<T> {}
unsafe impl<T: Copy + Send> Send for Ring<T> {}

impl<T: Copy> Ring<T> {
    /// A ring of [`capacity_for`]`(capacity)` entries, every slot initialized to `zero`.
    pub fn new(capacity: usize, zero: T) -> Ring<T> {
        Ring::starting_at(capacity, 0, zero)
    }

    pub(crate) fn starting_at(capacity: usize, index: usize, zero: T) -> Ring<T> {
        let capacity = capacity_for(capacity);
        Ring {
            head: CachePadded(AtomicUsize::new(index)),
            tail: CachePadded(AtomicUsize::new(index)),
            dropped: CachePadded(AtomicU64::new(0)),
            mask: capacity - 1,
            slots: (0..capacity).map(|_| UnsafeCell::new(zero)).collect(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Entries dropped because the ring was full, since it was made.
    pub fn dropped(&self) -> u64 {
        self.dropped.0.load(Ordering::Relaxed)
    }

    fn slot(&self, index: usize) -> *mut T {
        // SAFETY: `index & mask` is always below `slots.len()`, which is `mask + 1`.
        unsafe { self.slots.get_unchecked(index & self.mask) }.get()
    }

    /// Append `entry` unless that would leave fewer than `reserve` free slots; in that case the entry is dropped and counted,
    /// and the entries already queued are kept (drop-newest). Returns whether the entry was queued. Never blocks.
    ///
    /// The device read thread pushes reports with `reserve` 1 and its single session-ending error entry with `reserve` 0, so
    /// the error always fits however far behind the consumer is.
    ///
    /// # Safety
    /// Only one thread at a time may call `push` on a ring.
    pub unsafe fn push(&self, entry: &T, reserve: usize) -> bool {
        let head = self.head.0.load(Ordering::Relaxed);
        // Acquire: the consumer has finished reading every slot below `tail` before the producer may write over it.
        let tail = self.tail.0.load(Ordering::Acquire);
        if head.wrapping_sub(tail) + reserve >= self.capacity() {
            self.dropped.0.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // SAFETY: slot `head` is outside the consumer's readable range `tail..head`, so only this thread touches it.
        unsafe { ptr::write(self.slot(head), *entry) };
        // Release: the slot's contents are visible to a consumer that sees the new head.
        self.head.0.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Move up to `max` of the oldest entries to `out`, in the order they were pushed, and return how many.
    ///
    /// # Safety
    /// Only one thread at a time may call `read_into` on a ring, and `out` must be valid for writing `max` entries.
    pub unsafe fn read_into(&self, out: *mut T, max: usize) -> usize {
        let tail = self.tail.0.load(Ordering::Relaxed);
        // Acquire: pairs with the producer's Release store, so every slot below `head` is fully written.
        let head = self.head.0.load(Ordering::Acquire);
        let count = head.wrapping_sub(tail).min(max);
        for i in 0..count {
            // SAFETY: slot `tail + i` lies in `tail..head`, which the producer does not write until `tail` moves past it; the
            // caller promised `out` has room for `max >= count` entries.
            unsafe { ptr::write(out.add(i), ptr::read(self.slot(tail.wrapping_add(i)))) };
        }
        // Release: the reads above are complete before the producer may reuse these slots.
        self.tail.0.store(tail.wrapping_add(count), Ordering::Release);
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct E {
        n: u64,
        check: [u8; 40],
    }

    fn e(n: u64) -> E {
        let mut check = [0u8; 40];
        for (i, b) in check.iter_mut().enumerate() {
            *b = (n as u8).wrapping_add(i as u8);
        }
        E { n, check }
    }

    const ZERO: E = E { n: 0, check: [0; 40] };

    fn push(ring: &Ring<E>, x: &E) -> bool {
        // SAFETY: single producer in these tests.
        unsafe { ring.push(x, 0) }
    }

    fn drain(ring: &Ring<E>, max: usize) -> Vec<E> {
        let mut out = vec![ZERO; max];
        // SAFETY: single-threaded test; `out` holds `max` entries.
        let count = unsafe { ring.read_into(out.as_mut_ptr(), max) };
        out.truncate(count);
        out
    }

    #[test]
    fn capacity_is_defaulted_clamped_and_rounded_to_a_power_of_two() {
        assert_eq!(capacity_for(0), DEFAULT_CAPACITY);
        assert_eq!(capacity_for(1), MIN_CAPACITY);
        assert_eq!(capacity_for(17), 32);
        assert_eq!(capacity_for(256), 256);
        assert_eq!(capacity_for(3000), 4096);
        assert_eq!(capacity_for(usize::MAX), MAX_CAPACITY);
        assert_eq!(Ring::new(0, ZERO).capacity(), DEFAULT_CAPACITY);
    }

    #[test]
    fn entries_come_back_in_order_across_many_wrap_arounds() {
        let ring = Ring::new(16, ZERO);
        let (mut next_in, mut next_out) = (0u64, 0u64);
        // Uneven batch sizes so reads and writes land at every slot offset.
        for round in 0..500u64 {
            for _ in 0..(round % 7 + 1) {
                assert!(push(&ring, &e(next_in)));
                next_in += 1;
            }
            for got in drain(&ring, (round % 5 + 1) as usize) {
                assert_eq!(got, e(next_out));
                next_out += 1;
            }
            if next_in - next_out > 8 {
                for got in drain(&ring, 16) {
                    assert_eq!(got, e(next_out));
                    next_out += 1;
                }
            }
        }
        for got in drain(&ring, 16) {
            assert_eq!(got, e(next_out));
            next_out += 1;
        }
        assert_eq!(next_out, next_in);
        assert!(next_in as usize > 100 * ring.capacity());
        assert_eq!(ring.dropped(), 0);
    }

    #[test]
    fn indexes_wrap_at_usize_max() {
        let ring = Ring::starting_at(16, usize::MAX - 5, ZERO);
        for n in 0..12 {
            assert!(push(&ring, &e(n)));
        }
        assert_eq!(drain(&ring, 16), (0..12).map(e).collect::<Vec<_>>());
        for n in 12..28 {
            assert!(push(&ring, &e(n)));
        }
        assert!(!push(&ring, &e(99)));
        assert_eq!(drain(&ring, 32), (12..28).map(e).collect::<Vec<_>>());
        assert_eq!(ring.dropped(), 1);
    }

    #[test]
    fn a_full_ring_drops_new_entries_counts_them_keeps_old_ones_and_recovers() {
        let ring = Ring::new(16, ZERO);
        for n in 0..20 {
            assert_eq!(push(&ring, &e(n)), n < 16);
        }
        assert_eq!(ring.dropped(), 4);
        assert_eq!(drain(&ring, 4), (0..4).map(e).collect::<Vec<_>>());
        for n in 100..104 {
            assert!(push(&ring, &e(n)));
        }
        assert!(!push(&ring, &e(104)));
        assert_eq!(ring.dropped(), 5);
        let expected: Vec<E> = (4..16).chain(100..104).map(e).collect();
        assert_eq!(drain(&ring, 64), expected);
        assert!(drain(&ring, 64).is_empty());
    }

    #[test]
    fn a_reserved_slot_keeps_room_for_the_final_error_entry() {
        let ring = Ring::new(16, ZERO);
        // Reports are pushed with reserve 1: only 15 fit.
        for n in 0..20 {
            // SAFETY: single producer.
            assert_eq!(unsafe { ring.push(&e(n), 1) }, n < 15);
        }
        assert_eq!(ring.dropped(), 5);
        // The error entry (reserve 0) still fits in the last slot; nothing fits after it.
        // SAFETY: single producer.
        unsafe {
            assert!(ring.push(&e(999), 0));
            assert!(!ring.push(&e(1000), 0));
        }
        assert_eq!(ring.dropped(), 6);
        let got = drain(&ring, 32);
        assert_eq!(got.len(), 16);
        assert_eq!(got[..15], (0..15).map(e).collect::<Vec<_>>()[..]);
        assert_eq!(got[15], e(999));
    }

    #[test]
    fn reading_nothing_or_into_zero_room_moves_nothing() {
        let ring = Ring::new(16, ZERO);
        assert!(drain(&ring, 8).is_empty());
        assert!(push(&ring, &e(1)));
        assert!(drain(&ring, 0).is_empty());
        assert_eq!(drain(&ring, 8), vec![e(1)]);
    }

    #[test]
    fn one_producer_and_one_consumer_on_different_threads_lose_nothing_unaccounted() {
        const COUNT: u64 = 2_000_000;
        let ring = Arc::new(Ring::new(64, ZERO));
        let producer_ring = Arc::clone(&ring);
        let producer = std::thread::spawn(move || {
            for n in 1..=COUNT {
                // SAFETY: this is the only producer thread.
                unsafe { producer_ring.push(&e(n), 1) };
            }
        });
        let mut received = 0u64;
        let mut last = 0u64;
        let mut out = vec![ZERO; 64];
        loop {
            let finished = producer.is_finished();
            // SAFETY: this is the only consumer thread; `out` holds 64 entries.
            let count = unsafe { ring.read_into(out.as_mut_ptr(), out.len()) };
            for got in &out[..count] {
                // Strictly increasing: nothing duplicated or reordered, and every entry is whole (no torn writes).
                assert!(got.n > last);
                assert_eq!(*got, e(got.n));
                last = got.n;
            }
            received += count as u64;
            if finished && count == 0 {
                break;
            }
        }
        producer.join().unwrap();
        assert_eq!(received + ring.dropped(), COUNT);
    }
}
