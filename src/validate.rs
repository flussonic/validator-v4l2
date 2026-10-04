// SPDX-License-Identifier: MIT
use crate::{
    device::{Layout, Mode},
    pattern::*,
    Result,
};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct Stats {
    pub frames: u64,
    pub gaps: u64,
    pub errors: Vec<String>,
    pub failure_count: u64,
    pub audio_samples: u64,
    pub present: u32,
    pub measured: u32,
    pub nonpcm: u32,
    pub flags: u32,
    pub anc: BTreeMap<String, u64>,
    pub vbi_frames: u64,
    pub hw_frames: u64,
    pub source_events: u64,
    last_seq: Option<u32>,
    last_time: Option<u64>,
    last_hw: Option<u64>,
    last_marker: Option<u64>,
    phase: Option<u64>,
    observed_audio: bool,
    picture: Option<crate::sapsan::Picture>,
    encoded_phase: Option<u64>,
}
impl Stats {
    pub fn fail(&mut self, msg: impl Into<String>) {
        self.failure_count += 1;
        if self.errors.len() < 20 {
            self.errors.push(msg.into());
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        p: [&[u8]; 5],
        l: Layout,
        m: Mode,
        seq: u32,
        flags: u32,
        time: u64,
        expect: Option<&Config>,
    ) -> Result<()> {
        self.frames += 1;
        if flags & 0x40 != 0 {
            self.fail("V4L2_BUF_FLAG_ERROR");
        }
        if let Some(last) = self.last_seq {
            let step = seq.wrapping_sub(last);
            if step != 1 {
                let lost = step.saturating_sub(1);
                self.gaps += lost as u64;
                self.fail(format!("sequence discontinuity {last}->{seq}"));
            }
        }
        self.last_seq = Some(seq);
        if let Some(last) = self.last_time {
            if time <= last {
                self.fail("non-monotonic V4L2 timestamp");
            }
        }
        self.last_time = Some(time);
        if p[0].len() < l.stride as usize * l.height as usize {
            return Err("short video plane".into());
        }
        if p[3].len() < 128 {
            return Err("short metadata plane".into());
        }
        let meta = p[3];
        if get32(meta, 0)? != META_MAGIC || get32(meta, 4)? != 4 {
            return Err("metadata magic/version mismatch".into());
        }
        let vendor = get32(meta, 116)? as usize;
        if vendor > meta.len() - 128 {
            return Err("vendor metadata truncated".into());
        }
        for off in (44..116).step_by(4) {
            if get32(meta, off)? != 0 {
                self.fail("nonzero reserved metadata");
                break;
            }
        }
        let mf = get32(meta, 8)?;
        self.flags |= mf;
        if mf & 48 == 48 {
            self.fail("HLG and PQ both set");
        }
        if get32(meta, 12)? != 0 {
            self.fail("frame CRC errors");
        }
        let hw = u64::from_le_bytes(meta[16..24].try_into().unwrap());
        if hw != 0 {
            self.hw_frames += 1;
            if self.last_hw.is_some_and(|v| hw <= v) {
                self.fail("non-monotonic hardware timestamp");
            }
            self.last_hw = Some(hw);
        }
        let present = get32(meta, 24)?;
        let nonpcm = get32(meta, 40)?;
        if present & !0xffff != 0 || nonpcm & !present != 0 {
            self.fail("invalid audio masks");
        }
        self.present |= present;
        self.nonpcm |= nonpcm;
        let count = (get32(meta, 28)? as usize)
            .checked_add(get32(meta, 32)? as usize)
            .ok_or("sample count overflow")?;
        if p[1].len() % 64 != 0 {
            return Err("audio bytesused not divisible by 64".into());
        }
        if count * 64 > p[1].len() {
            return Err("audio sample count exceeds bytesused".into());
        }
        let declared = if count == 0 { p[1].len() / 64 } else { count };
        let rate = get32(meta, 36)?;
        if declared != 0 && rate != 0 && rate != 48000 {
            self.fail(format!("audio rate {rate}, expected 48000"));
        }
        self.audio_samples += declared as u64;
        if expect.is_some() {
            let nominal = 48000. * m.den as f64 / m.num as f64;
            if (declared as f64 - nominal).abs() > 1.01 {
                self.fail(format!("audio cadence {declared}, expected {nominal:.3}"));
            }
        }

        let packet_list = packets(p[2])?;
        for p in &packet_list {
            *self
                .anc
                .entry(format!("{:02x}/{:02x}", p.did, p.sdid))
                .or_default() += 1;
            if p.line == 0 || p.line as u32 > m.total_lines {
                self.fail("ANC line outside timing");
            }
        }
        let rows = match m.total_lines {
            625 => 34,
            525 => 24,
            _ => 0,
        };
        if p[4].len() % 1440 != 0 || p[4].len() > rows * 1440 {
            return Err("invalid VBI plane length".into());
        }
        if !p[4].is_empty() {
            self.vbi_frames += 1;
        }
        let mark = if expect.is_some() {
            Some(marker(p[0], l)?)
        } else {
            None
        };
        if let (Some(c), Some(frame)) = (expect, mark) {
            check_video(p[0], l, frame, m)?;
            if self.picture.is_none() {
                self.picture = Some(crate::sapsan::Picture::new(l.width, l.height, m.num, m.den));
            }
            let picture = self.picture.as_mut().unwrap();
            picture.render(frame);
            if let Some((x, y, w, h)) = picture.frame_rect() {
                let expected = picture.image();
                let step = (l.width as usize / 640).min(l.height as usize / 360).max(1);
                for yy in (y..y + h).step_by(step) {
                    for xx in (x..x + w).step_by(step) {
                        let got = luma(p[0], l, xx as u32, yy as u32)?;
                        let want = expected[yy * l.width as usize + xx] as u16 * 4;
                        if got.abs_diff(want) > 16 {
                            return Err("Sapsan frame-counter text mismatch".into());
                        }
                    }
                }
            }

            if let Some(last) = self.last_marker {
                if frame != last.wrapping_add(1) {
                    self.fail(format!("picture marker discontinuity {last}->{frame}"));
                }
            }
            self.last_marker = Some(frame);
            if mf & 0x39 != c.flags_at(frame) & 0x39 {
                self.fail(format!(
                    "HDR/level flags {mf:#x}, expected {:#x}",
                    c.flags_at(frame)
                ));
            }
            if present & ((1 << c.channels) - 1) != (1 << c.channels) - 1 {
                self.fail(format!(
                    "audio present {present:#x}, expected {} channels",
                    c.channels
                ));
            }
            if c.nonpcm && nonpcm & 3 != 3 {
                self.fail("SMPTE 337M channels not detected");
            }
            if c.anc {
                for wanted in fixtures(frame, m) {
                    if !packet_list.iter().any(|p| {
                        p.did == wanted.did && p.sdid == wanted.sdid && p.data == wanted.data
                    }) {
                        self.fail(format!(
                            "missing/corrupt ANC {:02x}/{:02x} for picture {frame}",
                            wanted.did, wanted.sdid
                        ));
                    }
                }
            }
            if c.vbi && rows != 0 {
                if m.total_lines == 625 {
                    if let Err(e) = crate::teletext::check_vbi(p[4], m.total_lines, frame) {
                        self.fail(e);
                    }
                } else {
                    let mut v = vec![0; rows * 1440];
                    vbi(&mut v, m.total_lines, frame)?;
                    for line in [21, 284] {
                        let row = crate::teletext::row(m.total_lines, line).unwrap();
                        let range = row * 1440..(row + 1) * 1440;
                        if p[4].get(range.clone()) != Some(&v[range]) {
                            self.fail("VBI waveform mismatch");
                        }
                    }
                }
            }
        }
        let words = p[1];
        let mut active = 0;
        for ch in 0..16 {
            for sample in 0..declared {
                let v = get32(words, (sample * 16 + ch) * 4)? as i32 >> 8;
                if v != 0 {
                    active |= 1 << ch;
                    break;
                }
            }
        }
        self.measured |= active;
        if let Some(c) = expect {
            if c.nonpcm {
                if self.encoded_phase.is_none() {
                    for i in 0..declared {
                        if get32(words, i * 64)? >> 16 == 0xf872
                            && get32(words, i * 64 + 4)? >> 16 == 0x4e1f
                        {
                            self.encoded_phase = Some((1536 - i as u64 % 1536) % 1536);
                            break;
                        }
                    }
                }
                if let Some(phase) = self.encoded_phase {
                    let mut bad = 0;
                    for i in 0..declared {
                        for ch in 0..2 {
                            let got = get32(words, i * 64 + ch * 4)? & 0xffffff00;
                            if got != encoded_word(phase + i as u64, ch) {
                                bad += 1;
                            }
                        }
                    }
                    if bad != 0 {
                        self.fail(format!("{bad} SMPTE 337M transport payload mismatches"));
                    }
                    self.encoded_phase = Some((phase + declared as u64) % 1536);
                }
            }
            let begin = if c.nonpcm { 2 } else { 0 };
            if declared != 0 && !self.observed_audio {
                // Solve a common initial phase from all PCM channels, not just channel 1.
                let mut best = (u64::MAX, 0);
                for phase in 0..192 {
                    let mut score = 0u64;
                    for ch in begin..c.channels as usize {
                        for i in 0..declared.min(96) {
                            let a = get32(words, (i * 16 + ch) * 4)? as i32 >> 8;
                            score += a.abs_diff(tone(phase + i as u64, ch)) as u64;
                        }
                    }
                    if score < best.0 {
                        best = (score, phase);
                    }
                }
                self.phase = Some(best.1);
                self.observed_audio = true;
            }
            if let Some(phase) = self.phase {
                let mut bad = 0;
                for i in 0..declared {
                    for ch in begin..c.channels as usize {
                        let a = get32(words, (i * 16 + ch) * 4)? as i32 >> 8;
                        if a.abs_diff(tone(phase + i as u64, ch)) > 8192 {
                            bad += 1;
                        }
                    }
                }
                if bad != 0 {
                    self.fail(format!("{bad} PCM tone/continuity mismatches"));
                }
                self.phase = Some((phase + declared as u64) % 192);
            }
            if declared == 0 {
                self.fail("no audio samples");
            }
            for ch in c.channels as usize..16 {
                if active & (1 << ch) != 0 {
                    self.fail(format!("unexpected audio in channel {}", ch + 1));
                }
            }
        }
        Ok(())
    }
    pub fn finish(&mut self, expect: Option<&Config>) {
        if self.frames == 0 {
            self.fail("no frames validated");
        }
        if let Some(c) = expect {
            if self.audio_samples == 0 {
                self.fail("no audio received");
            }
            if c.nonpcm && self.encoded_phase.is_none() {
                self.fail("no valid SMPTE 337M preamble received");
            }
            if c.anc && self.anc.is_empty() {
                self.fail("no ANC received");
            }
        }
    }
}
