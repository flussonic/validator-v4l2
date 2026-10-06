// SPDX-License-Identifier: MIT
//! Known 188-byte MPEG-TS packets; packet validation spans V4L2 buffer boundaries.
use crate::{device::*, report, validate::Stats, Options, Result};
use std::time::{Duration, Instant};
const SIZE: usize = 188;
fn packet(id: u32, counter: u64) -> [u8; SIZE] {
    let mut p = [0u8; SIZE];
    p[..4].copy_from_slice(&[0x47, 0x1f, 0xfe, 0x10 | (counter as u8 & 15)]);
    p[4..12].copy_from_slice(b"VVASI001");
    p[12..16].copy_from_slice(&id.to_be_bytes());
    p[16..24].copy_from_slice(&counter.to_be_bytes());
    for (offset, byte) in p.iter_mut().enumerate().skip(24) {
        *byte = (counter.wrapping_mul(31).wrapping_add(offset as u64 * 17) ^ u64::from(id)) as u8;
    }
    p
}
fn fill(bytes: &mut [u8], id: u32, counter: &mut u64) -> u32 {
    let count = bytes.len() / SIZE;
    for chunk in bytes[..count * SIZE].chunks_exact_mut(SIZE) {
        chunk.copy_from_slice(&packet(id, *counter));
        *counter = counter.wrapping_add(1);
    }
    (count * SIZE) as u32
}
#[derive(Default)]
struct Receiver {
    pending: Vec<u8>,
    last: Option<u64>,
    aligned: bool,
}
impl Receiver {
    fn push(&mut self, bytes: &[u8], id: u32, stats: &mut Stats) -> bool {
        self.pending.extend_from_slice(bytes);
        if !self.aligned {
            let offset = self
                .pending
                .windows(12)
                .position(|p| p[0] == 0x47 && &p[4..12] == b"VVASI001");
            if let Some(offset) = offset {
                self.pending.drain(..offset);
                self.aligned = true;
            } else {
                let discard = self.pending.len().saturating_sub(11);
                self.pending.drain(..discard);
                return false;
            }
        }
        let count = self.pending.len() / SIZE;
        let mut found = false;
        stats.context("ASI packet payload");
        for bytes in self.pending[..count * SIZE].chunks_exact(SIZE) {
            let counter = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
            if bytes != packet(id, counter) {
                stats.fail("ASI packet payload/sync/identity mismatch");
                continue;
            }
            found = true;
            stats.checked("ASI packet payload");
            if self
                .last
                .is_some_and(|last| counter != last.wrapping_add(1))
            {
                stats.context("ASI packet continuity");
                stats.fail("ASI packet counter/continuity mismatch");
                stats.context("ASI packet payload");
            }
            self.last = Some(counter);
            stats.checked("ASI packet continuity");
        }
        self.pending.drain(..count * SIZE);
        found
    }
}
pub fn stream(o: &Options, output: bool) -> Result<bool> {
    let path = o.required("--device")?;
    let info = probe(path)?;
    if !info.asi() || (info.output != 0) != output {
        report::emit(
            o.report(),
            "SKIP",
            path,
            "ASI direction/MPEG transport is not advertised by this node",
            None,
        )?;
        return Ok(true);
    }
    if o.get("--memory", "mmap") != "mmap" {
        report::emit(
            o.report(),
            "SKIP",
            path,
            "ASI worker currently supports MMAP only; USERPTR/DMABUF are not tested",
            None,
        )?;
        return Ok(true);
    }
    let d = Device::open_asi(path, output)?;
    let id = o.u32("--probe-id", 0)?;
    let frames = o.number("--frames", 50)?;
    let duration = o.number("--duration", 0)?;
    let warmup = if output { 0 } else { o.u32("--warmup", 0)? };
    let mut counter = 0;
    for index in 0..d.count {
        let mut f = d.buffer(index)?;
        let used = if output {
            fill(f.planes_mut()[0], id, &mut counter)
        } else {
            0
        };
        f.len = [used, 0, 0, 0, 0];
        d.queue(&f)?;
    }
    d.start()?;
    let start = Instant::now();
    let mut stats = Stats::default();
    let mut rx = Receiver::default();
    let mut last_seq = None;
    let mut discarded = 0;
    stats.skip_check("stream continuity", "No completed buffers");
    stats.skip_check(
        "ASI 204-byte packets",
        "This scenario validates 188-byte TS only; RS(204) is not covered",
    );
    if !output {
        for name in ["ASI packet payload", "ASI packet continuity"] {
            stats.skip_check(name, "No known TS packets received");
        }
    }
    while stats.frames < frames
        && !crate::stop_requested()
        && (duration == 0 || start.elapsed() < Duration::from_secs(duration))
    {
        let mut f = d.next(o.u32("--timeout-ms", 3000)?)?;
        if discarded < warmup {
            discarded += 1;
        } else {
            stats.frames += 1;
            stats.context("stream continuity");
            if f.flags & 0x40 != 0 {
                stats.fail("V4L2_BUF_FLAG_ERROR");
            }
            if last_seq.is_some_and(|last: u32| f.sequence != last.wrapping_add(1)) {
                stats.fail("ASI V4L2 sequence discontinuity");
            }
            last_seq = Some(f.sequence);
            stats.checked("stream continuity");
            if !output && (o.has("--expect") || id != 0) {
                let found = rx.push(f.planes()[0], id, &mut stats);
                if found {
                    stats.probe_frames += 1;
                } else {
                    stats.probe_frames = 0;
                }
            }
        }
        if output {
            f = d.buffer(f.index)?;
        }
        let used = if output {
            fill(f.planes_mut()[0], id, &mut counter)
        } else {
            0
        };
        f.len = [used, 0, 0, 0, 0];
        d.queue(&f)?;
    }
    if !output && o.has("--expect") && rx.last.is_none() {
        stats.context("ASI packet payload");
        stats.fail("No known ASI TS packets received");
    }
    stats.finish(None);
    let status = if stats.failure_count != 0 {
        "FAIL"
    } else if output || !o.has("--expect") {
        "OBSERVED"
    } else {
        "PASS"
    };
    report::emit_with_boards(
        o.report(),
        status,
        path,
        &format!(
            "ASI MPEG-TS 188-byte packets, elapsed={:.3}s startup_discarded={discarded}",
            start.elapsed().as_secs_f64()
        ),
        Some(&stats),
        &[report::device_board(&info, path)],
    )?;
    Ok(stats.failure_count == 0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_checks_span_buffers_and_detect_corruption_and_gaps() {
        let mut rx = Receiver::default();
        let mut stats = Stats::default();
        let a = packet(7, 100);
        assert!(!rx.push(&a[..53], 7, &mut stats));
        assert!(rx.push(&a[53..], 7, &mut stats));
        assert!(rx.push(&packet(7, 101), 7, &mut stats));
        assert_eq!(stats.failure_count, 0);
        assert!(rx.push(&packet(7, 103), 7, &mut stats));
        assert_eq!(stats.checks["ASI packet continuity"].failures, 1);
        let mut bad = packet(7, 104);
        bad[100] ^= 1;
        rx.push(&bad, 7, &mut stats);
        assert_eq!(stats.checks["ASI packet payload"].failures, 1);
        rx.push(&packet(8, 105), 7, &mut stats);
        assert_eq!(stats.checks["ASI packet payload"].failures, 2);
    }
}
