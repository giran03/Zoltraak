//! Byte-range segmentation and work stealing.
//!
//! A download is split into [`Chunk`]s, each fetched by its own ranged request
//! and written straight to its offset in the preallocated target file — there
//! is no separate merge pass.

use serde::{Deserialize, Serialize};

/// A half-open byte range `[start, end)` plus how much of it is already on disk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub start: u64,
    pub end: u64,
    /// Bytes durably written to the file. This is what gets persisted, so it
    /// must never run ahead of the disk.
    pub downloaded: u64,
    /// Bytes a worker has reserved but not yet written.
    ///
    /// Runtime-only, and deliberately not serialized: a crash between reserving
    /// and writing must not leave a sidecar claiming bytes that never landed,
    /// which would resume over a hole and silently corrupt the file.
    #[serde(skip)]
    pub claimed: u64,
}

impl Chunk {
    pub fn new(start: u64, end: u64) -> Self {
        debug_assert!(end >= start, "chunk end must not precede start");
        Self {
            start,
            end,
            downloaded: 0,
            claimed: 0,
        }
    }

    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes neither written nor reserved — i.e. still up for grabs.
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.cursor())
    }

    /// Completion is measured in *written* bytes; reserved-but-unwritten bytes
    /// do not count.
    pub fn is_complete(&self) -> bool {
        self.start + self.downloaded >= self.end
    }

    /// The next byte to request. Includes reserved bytes so a stealer never
    /// hands out a region that a peer is mid-write on.
    pub fn cursor(&self) -> u64 {
        self.start + self.downloaded + self.claimed
    }

    /// Split off the back half of this chunk's *remaining* work.
    ///
    /// Returns `None` when there is too little left to be worth another
    /// connection — splitting a nearly-finished chunk costs a round trip and
    /// saves nothing.
    pub fn split(&mut self, min_remaining: u64) -> Option<Chunk> {
        let remaining = self.remaining();
        if remaining < min_remaining.max(2) {
            return None;
        }
        let midpoint = self.cursor() + remaining / 2;
        let tail = Chunk::new(midpoint, self.end);
        self.end = midpoint;
        Some(tail)
    }
}

/// Divide `total` bytes into `connections` contiguous chunks.
///
/// The final chunk absorbs the remainder, so the chunks always exactly cover
/// `0..total` with no gap and no overlap.
pub fn plan(total: u64, connections: u8) -> Vec<Chunk> {
    let connections = connections.max(1) as u64;
    if total == 0 {
        return vec![Chunk::new(0, 0)];
    }

    let n = connections.min(total);
    let per = total / n;

    (0..n)
        .map(|i| {
            let start = i * per;
            let end = if i == n - 1 { total } else { start + per };
            Chunk::new(start, end)
        })
        .collect()
}

/// Pick the chunk with the most work left, for a finished worker to steal from.
pub fn steal_target(chunks: &mut [Chunk]) -> Option<&mut Chunk> {
    chunks
        .iter_mut()
        .filter(|c| !c.is_complete())
        .max_by_key(|c| c.remaining())
}

/// Take half the busiest chunk's remaining work as a new chunk.
///
/// Returns the index of the newly appended chunk, or `None` when no chunk has
/// enough left to be worth splitting. The victim's `end` shrinks, so the worker
/// currently streaming it must re-read its end and stop early.
pub fn steal_from(chunks: &mut Vec<Chunk>, min_remaining: u64) -> Option<usize> {
    let tail = steal_target(chunks)?.split(min_remaining)?;
    chunks.push(tail);
    Some(chunks.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers_exactly(chunks: &[Chunk], total: u64) -> bool {
        if chunks.first().map(|c| c.start) != Some(0) {
            return false;
        }
        if chunks.last().map(|c| c.end) != Some(total) {
            return false;
        }
        chunks.windows(2).all(|w| w[0].end == w[1].start)
    }

    #[test]
    fn plan_covers_the_whole_file() {
        for (total, conns) in [(1000, 4), (1001, 4), (7, 3), (1, 8), (1 << 30, 16)] {
            let chunks = plan(total, conns);
            assert!(
                covers_exactly(&chunks, total),
                "gap or overlap for total={total} conns={conns}: {chunks:?}"
            );
        }
    }

    #[test]
    fn plan_never_makes_more_chunks_than_bytes() {
        // 8 connections over 3 bytes would otherwise yield empty chunks.
        let chunks = plan(3, 8);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn plan_remainder_lands_in_the_final_chunk() {
        let chunks = plan(10, 3);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], Chunk::new(0, 3));
        assert_eq!(chunks[1], Chunk::new(3, 6));
        assert_eq!(chunks[2], Chunk::new(6, 10));
    }

    #[test]
    fn zero_length_file_yields_one_empty_chunk() {
        let chunks = plan(0, 8);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_empty());
    }

    #[test]
    fn split_takes_the_back_half_of_remaining_work() {
        let mut c = Chunk::new(0, 100);
        c.downloaded = 40;
        let tail = c.split(8).expect("60 bytes left is worth splitting");

        assert_eq!(c.start, 0);
        assert_eq!(c.end, 70);
        assert_eq!(c.downloaded, 40);
        assert_eq!(tail, Chunk::new(70, 100));
        // The pair still covers the original range contiguously.
        assert_eq!(c.end, tail.start);
    }

    #[test]
    fn split_declines_when_too_little_is_left() {
        let mut c = Chunk::new(0, 100);
        c.downloaded = 96;
        assert_eq!(c.split(16), None);
        assert_eq!(c.end, 100, "declined split must not mutate the chunk");
    }

    #[test]
    fn steal_target_picks_the_busiest_incomplete_chunk() {
        let mut chunks = vec![Chunk::new(0, 10), Chunk::new(10, 100), Chunk::new(100, 120)];
        chunks[0].downloaded = 10; // complete
        chunks[1].downloaded = 5; // 85 left
        chunks[2].downloaded = 0; // 20 left

        let target = steal_target(&mut chunks).expect("two chunks are incomplete");
        assert_eq!(target.start, 10);
    }

    #[test]
    fn steal_target_is_none_when_everything_is_done() {
        let mut chunks = vec![Chunk::new(0, 10)];
        chunks[0].downloaded = 10;
        assert!(steal_target(&mut chunks).is_none());
    }

    #[test]
    fn steal_from_appends_the_tail_and_shrinks_the_victim() {
        let mut chunks = vec![Chunk::new(0, 100), Chunk::new(100, 400)];
        chunks[0].downloaded = 100; // done
        chunks[1].downloaded = 100; // 200 left

        let new = steal_from(&mut chunks, 16).expect("200 bytes is worth splitting");
        assert_eq!(new, 2);
        assert_eq!(chunks[1].end, 300, "victim shrank");
        assert_eq!(chunks[2], Chunk::new(300, 400));
        // Coverage is preserved: victim end meets the new chunk's start.
        assert_eq!(chunks[1].end, chunks[2].start);
    }

    #[test]
    fn claimed_bytes_are_not_offered_to_a_stealer() {
        // The regression that mattered: without counting reserved bytes, a
        // stealer could hand out a region a peer was mid-write on, so both
        // wrote it and progress exceeded the file size.
        let mut chunks = vec![Chunk::new(0, 1000)];
        chunks[0].downloaded = 100;
        chunks[0].claimed = 400; // a write of 400 bytes is in flight

        assert_eq!(chunks[0].cursor(), 500, "cursor must clear in-flight bytes");
        assert_eq!(chunks[0].remaining(), 500);

        let new = steal_from(&mut chunks, 16).unwrap();
        assert!(
            chunks[new].start >= 500,
            "stole into a region being written: {:?}",
            chunks[new]
        );
    }

    #[test]
    fn completion_ignores_reserved_bytes() {
        // A chunk whose bytes are merely reserved is not done — counting them
        // would let the engine declare success before the data hit the disk.
        let mut c = Chunk::new(0, 100);
        c.downloaded = 60;
        c.claimed = 40;
        assert!(!c.is_complete());

        c.claimed = 0;
        c.downloaded = 100;
        assert!(c.is_complete());
    }

    #[test]
    fn claimed_is_not_persisted() {
        // Serializing in-flight bytes would make a crashed run resume past
        // data that was never written.
        let mut c = Chunk::new(0, 100);
        c.downloaded = 30;
        c.claimed = 25;

        let restored: Chunk = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(restored.downloaded, 30);
        assert_eq!(restored.claimed, 0, "claimed must not survive a round trip");
        assert_eq!(restored.cursor(), 30);
    }

    #[test]
    fn steal_from_declines_when_nothing_is_worth_splitting() {
        let mut chunks = vec![Chunk::new(0, 100)];
        chunks[0].downloaded = 99;
        assert_eq!(steal_from(&mut chunks, 16), None);
        assert_eq!(chunks.len(), 1, "no phantom chunk appended");
    }

    #[test]
    fn repeated_stealing_never_loses_or_duplicates_bytes() {
        // Simulates fast workers repeatedly stealing from a slow one; the union
        // of all chunks must still be exactly 0..1000.
        let mut chunks = plan(1000, 2);
        chunks[0].downloaded = chunks[0].len(); // worker 0 finished early

        for _ in 0..6 {
            if let Some(i) = steal_from(&mut chunks, 8) {
                // Pretend the new worker immediately completes a little of it.
                chunks[i].downloaded = chunks[i].len().min(4);
            }
        }

        let mut ranges: Vec<(u64, u64)> = chunks.iter().map(|c| (c.start, c.end)).collect();
        ranges.sort_unstable();
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 1000);
        for w in ranges.windows(2) {
            assert_eq!(w[0].1, w[1].0, "gap or overlap in {ranges:?}");
        }
    }
}
