//! Port of `PacketRingBuffer.cs`: the rolling "last N seconds" store of encoded packets.
//!
//! Keyframe-aware trimming: a clip must start on a keyframe, so one keyframe older than the cutoff is
//! always kept. Thread-safe: the encoder thread calls [`PacketRing::add`], the save thread snapshots.
//!
//! Optimisation vs C#: C# used a `LinkedList` (one heap node per packet) and walked the keyframe queue
//! to find the second keyframe on every add. Here packets and keyframes live in `VecDeque`s addressed by
//! a monotonically increasing sequence number: same semantics, O(1) amortised, no per-packet node alloc.
//! Snapshots share packet bytes (`Arc<[u8]>`) instead of copying them.

use lag_ffi::media::EncodedPacket;
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

/// Start may precede the companion (audio) streams by this much without re-alignment.
const ALIGN_TOLERANCE_US: i64 = 500_000;
/// The keyframe a clip starts on may lie this far before the buffer window; older means the picture stood
/// still (no frames) across the window edge.
const MAX_LEAD_US: i64 = 3_000_000;

struct Inner {
    packets: VecDeque<EncodedPacket>,
    /// Sequence number of `packets[0]`.
    head_seq: u64,
    /// Sequence numbers of keyframes still in `packets`, oldest first.
    keyframes: VecDeque<u64>,
}

impl Inner {
    fn at(&self, seq: u64) -> &EncodedPacket {
        &self.packets[(seq - self.head_seq) as usize]
    }
    fn next_seq(&self) -> u64 {
        self.head_seq + self.packets.len() as u64
    }
}

pub struct PacketRing {
    buffer_us: i64,
    inner: Mutex<Inner>,
}

impl PacketRing {
    /// `buffer_seconds` is clamped to at least 5 s (as in C#).
    pub fn new(buffer_seconds: f64) -> Self {
        Self {
            buffer_us: (buffer_seconds.max(5.0) * 1_000_000.0) as i64,
            inner: Mutex::new(Inner { packets: VecDeque::new(), head_seq: 0, keyframes: VecDeque::new() }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock must not take the replay buffer down with it.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn buffer_us(&self) -> i64 {
        self.buffer_us
    }

    /// Approximate buffered span in seconds (newest - oldest pts).
    pub fn buffered_seconds(&self) -> f64 {
        let g = self.lock();
        match (g.packets.front(), g.packets.back()) {
            (Some(a), Some(b)) => (b.pts_us - a.pts_us) as f64 / 1_000_000.0,
            _ => 0.0,
        }
    }

    pub fn oldest_pts_us(&self) -> Option<i64> {
        self.lock().packets.front().map(|p| p.pts_us)
    }

    pub fn len(&self) -> usize {
        self.lock().packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total bytes held (for memory diagnostics).
    pub fn bytes(&self) -> usize {
        self.lock().packets.iter().map(|p| p.data.len()).sum()
    }

    pub fn add(&self, packet: EncodedPacket) {
        let mut g = self.lock();
        let seq = g.next_seq();
        let cutoff = packet.pts_us - self.buffer_us;
        if packet.is_keyframe {
            g.keyframes.push_back(seq);
        }
        g.packets.push_back(packet);
        // Drop the oldest keyframe segment while the NEXT keyframe already covers the cutoff.
        while g.keyframes.len() >= 2 {
            let second = g.keyframes[1];
            if g.at(second).pts_us > cutoff {
                break;
            }
            g.keyframes.pop_front();
            let drop = (second - g.head_seq) as usize;
            g.packets.drain(..drop);
            g.head_seq = second;
        }
    }

    /// Replay packets re-based to start at 0, plus the absolute pts they were re-based from.
    /// From the latest keyframe at/before (newest - N s), or the earliest keyframe, to the newest.
    /// With `min_start_us` (oldest pts the audio rings still hold), a start that would leave more than
    /// 0.5 s of audio-less head is advanced to the first keyframe the audio covers.
    pub fn snapshot(&self, min_start_us: Option<i64>) -> (Vec<EncodedPacket>, i64) {
        let g = self.lock();
        let Some(newest) = g.packets.back().map(|p| p.pts_us) else {
            return (Vec::new(), 0);
        };
        let cutoff = newest - self.buffer_us;
        let mut start = None;
        for &kf in &g.keyframes {
            if g.at(kf).pts_us <= cutoff {
                start = Some(kf);
            } else {
                break;
            }
        }
        let Some(mut start) = start.or_else(|| g.keyframes.front().copied()) else {
            return (Vec::new(), 0); // no keyframe yet: nothing decodable
        };
        // A still picture across the window edge (a static window sends no frames): the last keyframe before
        // the cutoff can be minutes old, and the clip came out minutes longer than the buffer (owner,
        // 2026-11-20: 16 min with a 2 min buffer). A lone keyframe (the next packet starts a new GOP) is the
        // picture that stood there, so it is shown from the cutoff on; a longer GOP is skipped to the first
        // keyframe inside the window.
        let mut still_from: Option<i64> = None;
        let lead = cutoff - g.at(start).pts_us;
        if lead > MAX_LEAD_US {
            let next = start + 1;
            if next >= g.next_seq() || g.at(next).is_keyframe {
                still_from = Some(cutoff);
            } else if let Some(&kf) = g.keyframes.iter().find(|&&kf| g.at(kf).pts_us > cutoff) {
                start = kf;
            }
            log::info!(
                "[PacketRingBuffer] picture stood still {:.1}s before the buffer window: clip kept to the window.",
                lead as f64 / 1_000_000.0
            );
        }
        if let Some(min_start) = min_start_us {
            let start_pts = still_from.unwrap_or(g.at(start).pts_us);
            if min_start - start_pts > ALIGN_TOLERANCE_US {
                if let Some(&kf) = g.keyframes.iter().find(|&&kf| g.at(kf).pts_us >= min_start) {
                    log::info!(
                        "[PacketRingBuffer] start advanced {:.1}s to align with audio.",
                        (g.at(kf).pts_us - start_pts) as f64 / 1_000_000.0
                    );
                    start = kf;
                    still_from = None;
                }
            }
        }
        let from = (start - g.head_seq) as usize;
        if let Some(base) = still_from {
            let first = g.at(start);
            let mut clip = vec![first.shifted(-first.pts_us)];
            clip.extend(g.packets.range(from + 1..).map(|p| p.shifted(-base)));
            return (clip, base);
        }
        let base = g.at(start).pts_us;
        (g.packets.range(from..).map(|p| p.shifted(-base)).collect(), base)
    }

    /// Every packet at/after `abs_base_us`, re-based to that origin (audio tracks).
    pub fn snapshot_rebased_from(&self, abs_base_us: i64) -> Vec<EncodedPacket> {
        let g = self.lock();
        g.packets.iter().filter(|p| p.pts_us >= abs_base_us).map(|p| p.shifted(-abs_base_us)).collect()
    }

    /// Every packet with ABSOLUTE timestamps (carry-over of a finished session's tail).
    pub fn snapshot_absolute(&self) -> Vec<EncodedPacket> {
        self.lock().packets.iter().cloned().collect()
    }

    pub fn clear(&self) {
        let mut g = self.lock();
        let next = g.next_seq();
        g.packets.clear();
        g.keyframes.clear();
        g.head_seq = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(pts_ms: i64, key: bool) -> EncodedPacket {
        EncodedPacket::new(vec![0u8; 4], pts_ms * 1000, pts_ms * 1000, key)
    }

    /// 10 s buffer, keyframe every 2 s, 100 fps for 30 s.
    fn filled() -> PacketRing {
        let r = PacketRing::new(10.0);
        for i in 0..3000 {
            let t = i * 10;
            r.add(pkt(t, t % 2000 == 0));
        }
        r
    }

    #[test]
    fn keeps_one_keyframe_before_cutoff() {
        let r = filled();
        // newest 29.99 s, cutoff 19.99 s -> latest keyframe <= cutoff is 18 s; the ring starts there.
        assert_eq!(r.oldest_pts_us(), Some(18_000_000));
        let (clip, base) = r.snapshot(None);
        assert_eq!(base, 18_000_000);
        assert_eq!(clip[0].pts_us, 0);
        assert!(clip[0].is_keyframe);
        assert_eq!(clip.last().unwrap().pts_us, 29_990_000 - 18_000_000);
    }

    #[test]
    fn minimum_buffer_is_5_seconds() {
        let r = PacketRing::new(1.0);
        for i in 0..1000 {
            r.add(pkt(i * 10, i % 100 == 0));
        }
        assert!(r.buffered_seconds() >= 5.0);
    }

    #[test]
    fn aligns_start_with_audio() {
        let r = filled();
        let (clip, base) = r.snapshot(Some(21_000_000));
        assert_eq!(base, 22_000_000);
        assert!(clip[0].is_keyframe);
        // Within tolerance (0.5 s) -> unchanged.
        let (_, base2) = r.snapshot(Some(18_400_000));
        assert_eq!(base2, 18_000_000);
    }

    #[test]
    fn rebased_audio_drops_negative() {
        let r = filled();
        let a = r.snapshot_rebased_from(25_000_000);
        assert_eq!(a[0].pts_us, 0);
        assert_eq!(a.len(), 500);
    }

    /// 10 s buffer. A static window: one frame at 0 s, then nothing until 125 s, then 5 s of frames (each
    /// frame after the gap is a forced keyframe, then every 2 s).
    fn still_then_motion(lone: bool) -> PacketRing {
        let r = PacketRing::new(10.0);
        r.add(pkt(0, true));
        if !lone {
            r.add(pkt(10, false)); // the old GOP has more than its keyframe
        }
        for i in 0..500 {
            let t = 125_000 + i * 10;
            r.add(pkt(t, (t - 125_000) % 2000 == 0));
        }
        r
    }

    #[test]
    fn still_picture_does_not_stretch_the_clip() {
        // lone keyframe: shown from the cutoff (129.99 - 10 = 119.99 s) -> clip is exactly the buffer
        let (clip, base) = still_then_motion(true).snapshot(None);
        assert_eq!(base, 119_990_000);
        assert_eq!(clip[0].pts_us, 0);
        assert!(clip[0].is_keyframe);
        assert_eq!(clip[1].pts_us, 125_000_000 - 119_990_000);
        assert_eq!(clip.last().unwrap().pts_us, 10_000_000);
        // longer old GOP: starts at the first keyframe inside the window instead
        let (clip, base) = still_then_motion(false).snapshot(None);
        assert_eq!(base, 125_000_000);
        assert!(clip[0].is_keyframe && clip[0].pts_us == 0);
        assert_eq!(clip.len(), 500);
    }

    #[test]
    fn no_keyframe_no_clip() {
        let r = PacketRing::new(10.0);
        r.add(pkt(0, false));
        assert!(r.snapshot(None).0.is_empty());
    }

    #[test]
    fn clear_then_reuse() {
        let r = filled();
        r.clear();
        assert!(r.is_empty());
        r.add(pkt(0, true));
        r.add(pkt(10, false));
        assert_eq!(r.snapshot(None).0.len(), 2);
    }
}
