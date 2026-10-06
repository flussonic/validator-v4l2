// SPDX-License-Identifier: MIT
use crate::{
    device::{Layout, Mode},
    pattern::*,
    Result,
};
use std::collections::BTreeMap;

fn pcm_phase_offsets(
    words: &[u8],
    samples: usize,
    begin: usize,
    channels: usize,
    common: u64,
) -> Result<Option<String>> {
    let mut offsets = Vec::new();
    for ch in begin..channels {
        let mut best = (u64::MAX, i64::MAX, 0);
        for offset in -96i64..96 {
            let phase = (common as i64 + offset).rem_euclid(192) as u64;
            let mut score = 0u64;
            for i in 0..samples.min(96) {
                let got = get32(words, (i * 16 + ch) * 4)? as i32 >> 8;
                score += got.abs_diff(tone(phase + i as u64, ch)) as u64;
            }
            if (score, offset.abs()) < (best.0, best.1) {
                best = (score, offset.abs(), offset);
            }
        }
        let phase = (common as i64 + best.2).rem_euclid(192) as u64;
        for i in 0..samples {
            let got = get32(words, (i * 16 + ch) * 4)? as i32 >> 8;
            if got.abs_diff(tone(phase + i as u64, ch)) > 8192 {
                return Ok(None);
            }
        }
        if best.2 != 0 {
            offsets.push(format!("{}:{:+}", ch + 1, best.2));
        }
    }
    Ok((!offsets.is_empty()).then(|| offsets.join(", ")))
}

#[derive(Default)]
pub struct CheckResult {
    pub observations: u64,
    pub failures: u64,
    pub errors: Vec<String>,
    pub reason: String,
}

#[derive(Default)]
pub struct Stats {
    pub checks: BTreeMap<String, CheckResult>,
    context: String,
    pub probe_frames: u64,
    last_probe: Option<u64>,
    pub windowed_audio: bool,
    pub frames: u64,
    pub crc_errors: u64,
    pub crc_supported: bool,
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
    expected_audio: f64,
    channel_observations: [u64; 16],
}
impl Stats {
    pub fn context(&mut self, name: &str) {
        self.context = name.into();
    }
    pub fn checked(&mut self, name: &str) {
        self.checks.entry(name.into()).or_default().observations += 1;
    }
    pub fn skip_check(&mut self, name: &str, reason: &str) {
        self.checks.entry(name.into()).or_default().reason = reason.into();
    }
    pub fn expect_checks(&mut self, config: Option<&Config>, mode: Mode, output: bool) {
        for name in ["stream continuity", "driver counters"] {
            self.skip_check(name, "No completed measurements");
        }
        if output {
            return;
        }
        for name in [
            "five-plane ABI",
            "CRC",
            "audio metadata/cadence",
            "ANC structure",
        ] {
            self.skip_check(name, "Not reached; inspect stream errors");
        }
        for name in [
            "video pattern",
            "HDR/level metadata",
            "PCM payload",
            "encoded audio",
            "encoded audio metadata",
            "SD VBI",
        ] {
            self.skip_check(name, "Not reached; inspect stream errors");
        }
        self.skip_check("CRC", "CRC measurement support is unknown; a zero metadata field does not certify hardware CRC");
        if let Some(c) = config {
            let begin = if c.nonpcm { 2 } else { 0 };
            for channel in begin..c.channels {
                self.skip_check(
                    &format!("PCM channel {}", channel + 1),
                    "No tone samples checked for this channel",
                );
            }
            if c.nonpcm && c.channels <= 2 {
                self.skip_check(
                    "PCM payload",
                    "Encoded two-slot carrier; no PCM channels requested",
                );
            }
            if !c.nonpcm {
                self.skip_check("encoded audio", "PCM scenario");
                self.skip_check("encoded audio metadata", "PCM scenario");
            }
            if !c.anc {
                self.skip_check("ANC structure", "ANC fixture checks disabled");
            }
            if !c.vbi {
                self.skip_check("SD VBI", "VBI waveform checks disabled");
            } else if ![525, 625].contains(&mode.total_lines) {
                self.skip_check("SD VBI", "VBI is unavailable for non-SD timing");
            }
            for packet in fixture_packets(0, mode, c.scte104_fragments) {
                self.skip_check(
                    &format!("ANC {:02x}/{:02x}", packet.did, packet.sdid),
                    if c.anc {
                        "Not reached; no associated picture"
                    } else {
                        "ANC fixture checks disabled"
                    },
                );
            }
        } else {
            for name in [
                "video pattern",
                "HDR/level metadata",
                "PCM payload",
                "encoded audio",
                "SD VBI",
            ] {
                self.skip_check(name, "No known generated source");
            }
        }
    }
    pub fn observe_probe(&mut self, picture: &[u8], layout: Layout, id: u32) -> Result<()> {
        let frame = match marker(picture, layout) {
            Ok(frame) => frame,
            Err(_) => {
                self.last_probe = None;
                self.probe_frames = 0;
                return Ok(());
            }
        };
        if id == 0 || frame >> 32 != u64::from(id) {
            self.last_probe = None;
            self.probe_frames = 0;
        } else {
            if self.last_probe.is_some_and(|last| frame != last + 1) {
                self.probe_frames = 0;
            }
            self.probe_frames += 1;
            self.last_probe = Some(frame);
        }
        Ok(())
    }

    pub fn fail_check(&mut self, name: &str, msg: impl Into<String>) {
        let msg = msg.into();
        let result = self.checks.entry(name.into()).or_default();
        result.failures += 1;
        if result.errors.len() < 20 {
            result.errors.push(msg.clone());
        }
        self.failure_count += 1;
        if self.errors.len() < 20 {
            self.errors.push(msg);
        }
    }
    pub fn fail(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        let name = if let Some(rest) = msg.strip_prefix("missing/corrupt ANC ") {
            format!(
                "ANC {}",
                rest.split_whitespace().next().unwrap_or("payload")
            )
        } else if msg.contains("sysfs ") {
            "driver counters".into()
        } else if msg.contains("HDR/level") || msg.contains("HLG and PQ") {
            "HDR/level metadata".into()
        } else if msg.contains("CRC") {
            "CRC".into()
        } else if msg.contains("VBI") || msg.contains("teletext") {
            "SD VBI".into()
        } else if msg.contains("SMPTE 337") || msg.contains("E-AC-3") {
            "encoded audio".into()
        } else if msg.contains("PCM") || msg.contains("unexpected audio") {
            "PCM payload".into()
        } else if msg.contains("audio") {
            "audio metadata/cadence".into()
        } else if msg.contains("sequence")
            || msg.contains("timestamp")
            || msg.contains("BUF_FLAG")
            || msg.contains("no frames")
        {
            "stream continuity".into()
        } else if msg.contains("marker") || msg.contains("video mismatch") || msg.contains("Sapsan")
        {
            "video pattern".into()
        } else if msg.contains("ANC") {
            "ANC structure".into()
        } else if msg.contains("metadata") || msg.contains("plane") {
            "five-plane ABI".into()
        } else {
            self.context.clone()
        };
        self.fail_check(
            if name.is_empty() {
                "stream operation"
            } else {
                &name
            },
            msg,
        );
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
        self.context("stream continuity");
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
        self.checked("stream continuity");
        self.context("five-plane ABI");
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
        self.checked("five-plane ABI");
        self.flags |= mf;
        if mf & 48 == 48 {
            self.fail("HLG and PQ both set");
        }
        let crc = get32(meta, 12)?;
        self.crc_errors = self.crc_errors.saturating_add(u64::from(crc));
        if crc != 0 {
            self.fail("frame CRC errors");
        }
        if crc != 0 || self.crc_supported {
            self.checked("CRC");
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
        self.context("audio metadata/cadence");
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
            self.expected_audio += nominal;
            let tolerance = if self.windowed_audio {
                nominal * 0.25
            } else {
                1.01
            };
            if (declared as f64 - nominal).abs() > tolerance {
                self.fail(format!("audio cadence {declared}, expected {nominal:.3}"));
            }
        }

        self.checked("audio metadata/cadence");
        self.context("ANC structure");
        let check_anc = expect.map_or(true, |config| config.anc);
        let (packet_list, anc_parse_ok) = match packets(p[2]) {
            Ok(packets) => (packets, true),
            Err(error) => {
                if check_anc {
                    self.fail_check("ANC structure", error);
                }
                (Vec::new(), false)
            }
        };
        for p in &packet_list {
            *self
                .anc
                .entry(format!("{:02x}/{:02x}", p.did, p.sdid))
                .or_default() += 1;
            if check_anc && (p.line == 0 || p.line as u32 > m.total_lines) {
                self.fail("ANC line outside timing");
            }
        }
        if check_anc {
            self.checked("ANC structure");
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
        self.context("video pattern");
        let mark = if expect.is_some() {
            match marker(p[0], l) {
                Ok(frame) => Some(frame),
                Err(error) => {
                    self.fail(error);
                    None
                }
            }
        } else {
            None
        };
        if let (Some(c), Some(frame)) = (expect, mark) {
            let picture_result = (|| -> Result<()> {
                check_video(p[0], l, frame, m)?;
                if self.picture.is_none() {
                    self.picture =
                        Some(crate::sapsan::Picture::new(l.width, l.height, m.num, m.den));
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
                                return Err(format!(
                                "Sapsan frame-counter text mismatch at {xx},{yy} for picture {frame}: {got}, expected {want}"
                            ));
                            }
                        }
                    }
                }

                Ok(())
            })();
            match picture_result {
                Ok(()) => self.checked("video pattern"),
                Err(error) => self.fail(error),
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
            self.checked("HDR/level metadata");
            if present & ((1 << c.channels) - 1) != (1 << c.channels) - 1 {
                self.fail(format!(
                    "audio present {present:#x}, expected {} channels",
                    c.channels
                ));
            }
            let preamble = (0..declared).any(|i| {
                get32(p[1], i * 64).is_ok_and(|v| v >> 16 == 0xf872)
                    && get32(p[1], i * 64 + 4).is_ok_and(|v| v >> 16 == 0x4e1f)
            });
            if c.nonpcm && preamble && nonpcm & 3 != 3 {
                self.fail_check(
                    "encoded audio metadata",
                    "SMPTE 337M channels 1/2 not marked as non-PCM despite a detected preamble",
                );
            }
            if c.nonpcm && nonpcm & 3 == 3 {
                self.checked("encoded audio metadata");
            }
            if c.anc && anc_parse_ok {
                for wanted in fixture_packets(frame, m, c.scte104_fragments) {
                    let name = format!("ANC {:02x}/{:02x}", wanted.did, wanted.sdid);
                    let same_type: Vec<_> = packet_list
                        .iter()
                        .filter(|p| p.did == wanted.did && p.sdid == wanted.sdid)
                        .collect();
                    if same_type.is_empty() {
                        self.fail_check(
                            &name,
                            format!(
                                "ANC {:02x}/{:02x}: expected packet absent for picture {frame}",
                                wanted.did, wanted.sdid
                            ),
                        );
                    } else if !same_type.iter().any(|p| p.data == wanted.data) {
                        self.fail_check(&name, format!("ANC {:02x}/{:02x}: packet present but payload does not match picture {frame}", wanted.did, wanted.sdid));
                    }
                    self.checked(&format!("ANC {:02x}/{:02x}", wanted.did, wanted.sdid));
                }
            }
            if c.vbi && rows != 0 {
                self.context("SD VBI");
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
                self.checked("SD VBI");
            }
        }
        let words = p[1];
        self.context("PCM payload");
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
                let period = encoded_period(c);
                if self.encoded_phase.is_none() {
                    for i in 0..declared {
                        if get32(words, i * 64)? >> 16 == 0xf872
                            && get32(words, i * 64 + 4)? >> 16 == 0x4e1f
                            && declared - i >= 16
                        {
                            let mut best = (u64::MAX, 0);
                            for start in (0..period).step_by(1536) {
                                let mut score = 0;
                                for j in i..declared.min(i + 128) {
                                    for ch in 0..2 {
                                        let got = get32(words, j * 64 + ch * 4)? & 0xffffff00;
                                        if got != encoded_word_for(start + (j - i) as u64, ch, c) {
                                            score += 1;
                                        }
                                    }
                                }
                                if score < best.0 {
                                    best = (score, start);
                                }
                            }
                            self.encoded_phase =
                                Some((best.1 + period - i as u64 % period) % period);
                            break;
                        }
                    }
                }
                if let Some(phase) = self.encoded_phase {
                    self.context("encoded audio");
                    let mut bad = 0;
                    for i in 0..declared {
                        for ch in 0..2 {
                            let got = get32(words, i * 64 + ch * 4)? & 0xffffff00;
                            if got != encoded_word_for(phase + i as u64, ch, c) {
                                bad += 1;
                            }
                        }
                    }
                    if bad != 0 {
                        self.fail(format!("{bad} SMPTE 337M transport payload mismatches"));
                    }
                    self.encoded_phase = Some((phase + declared as u64) % period);
                    self.checked("encoded audio");
                }
            }
            let begin = if c.nonpcm { 2 } else { 0 };
            let initial_pcm = declared != 0 && !self.observed_audio;
            if initial_pcm {
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
                self.context("PCM payload");
                let mut bad = 0;
                for ch in begin..c.channels as usize {
                    let name = format!("PCM channel {}", ch + 1);
                    let mut channel_bad = 0;
                    for i in 0..declared {
                        let got = get32(words, (i * 16 + ch) * 4)? as i32 >> 8;
                        if got.abs_diff(tone(phase + i as u64, ch)) > 8192 {
                            channel_bad += 1;
                        }
                    }
                    if present & (1 << ch) == 0 {
                        self.fail_check(
                            &name,
                            format!("PCM channel {}: absent from audio-present mask", ch + 1),
                        );
                    }
                    if declared == 0 {
                        self.fail_check(
                            &name,
                            format!("PCM channel {}: no audio samples received", ch + 1),
                        );
                    } else {
                        self.channel_observations[ch] += 1;
                        self.checked(&name);
                        if channel_bad != 0 {
                            self.fail_check(&name, format!("PCM channel {}: {channel_bad}/{declared} tone samples mismatch or lose continuity", ch + 1));
                        }
                    }
                    bad += channel_bad;
                }
                if bad != 0 {
                    let mut message = format!("{bad} PCM tone/continuity mismatches");
                    if initial_pcm {
                        // Diagnose a pure channel skew, while retaining the common
                        // phase for all checks and the failing verdict.
                        if let Some(offsets) =
                            pcm_phase_offsets(words, declared, begin, c.channels as usize, phase)?
                        {
                            message.push_str(&format!(
                                "; initial channel phase offsets (samples): {offsets}"
                            ));
                        }
                    }
                    self.fail(message);
                }
                self.phase = Some((phase + declared as u64) % 192);
                if c.channels as usize > begin {
                    self.checked("PCM payload");
                }
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
            if self.windowed_audio && self.frames != 0 {
                let allowance =
                    self.expected_audio * 0.001 + self.expected_audio / self.frames as f64 * 0.1;
                if (self.audio_samples as f64 - self.expected_audio).abs() > allowance {
                    self.fail(format!(
                        "audio window {} samples, expected {:.3} +/- {:.3}",
                        self.audio_samples, self.expected_audio, allowance
                    ));
                }
            }
            let begin = if c.nonpcm { 2 } else { 0 };
            for channel in begin..c.channels as usize {
                if self.channel_observations[channel] == 0 {
                    self.skip_check(
                        &format!("PCM channel {}", channel + 1),
                        "No valid samples checked; inspect audio cadence/ABI errors",
                    );
                }
            }
            if self.audio_samples == 0 {
                self.fail("no audio received");
            }
            if c.nonpcm && self.encoded_phase.is_none() {
                self.fail("no valid SMPTE 337M preamble received");
            }
            if c.nonpcm && self.nonpcm & 3 != 3 {
                self.fail_check(
                    "encoded audio metadata",
                    "No non-PCM channel mask for SDI slots 1/2 received",
                );
            }
            if c.anc && self.anc.is_empty() {
                self.fail("no ANC received");
            }
        }
    }
}

#[cfg(test)]
mod check_tests {
    use super::*;
    #[test]
    fn independent_checks_keep_failures_after_the_global_error_list_is_full() {
        let mut stats = Stats::default();
        stats.checked("PCM payload");
        for frame in 0..30 {
            stats.fail(format!("missing/corrupt ANC 60/60 for picture {frame}"));
        }
        stats.fail("HDR/level flags mismatch");
        assert_eq!(stats.errors.len(), 20);
        assert_eq!(stats.checks["ANC 60/60"].failures, 30);
        assert_eq!(stats.checks["HDR/level metadata"].failures, 1);
        assert_eq!(stats.checks["PCM payload"].failures, 0);
    }
}
