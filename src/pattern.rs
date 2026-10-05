// SPDX-License-Identifier: MIT
use crate::{
    device::{Layout, Mode},
    Result,
};
pub const META_MAGIC: u32 = u32::from_le_bytes(*b"SDI0");
pub const FORMATS: [&str; 7] = ["SDUY", "SDYU", "SDYV", "SD16", "SD10", "SDAR", "SDXU"];
pub fn fourcc(s: &str) -> Result<u32> {
    let b: [u8; 4] = s
        .as_bytes()
        .try_into()
        .map_err(|_| "fourcc must be four bytes")?;
    Ok(u32::from_le_bytes(b))
}
pub fn code(f: u32) -> String {
    String::from_utf8_lossy(&f.to_le_bytes()).into_owned()
}
pub fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn get32(b: &[u8], off: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(off..off + 4)
            .ok_or("truncated u32")?
            .try_into()
            .unwrap(),
    ))
}
pub fn word(v: u8) -> u16 {
    let p = (v.count_ones() & 1) as u16;
    v as u16 | (p << 8) | ((p ^ 1) << 9)
}
#[derive(Clone, Debug)]
pub struct Config {
    pub flags: u32,
    pub channels: u32,
    pub alternate: u32,
    pub pad: u32,
    pub no_meta: bool,
    pub nonpcm: bool,
    pub eac3: bool,
    pub anc: bool,
    pub vbi: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            flags: 0,
            channels: 16,
            alternate: 0,
            pad: 0,
            no_meta: false,
            nonpcm: false,
            eac3: false,
            anc: true,
            vbi: true,
        }
    }
}
impl Config {
    pub fn flags_at(&self, frame: u64) -> u32 {
        if self.alternate != 0 && frame / self.alternate as u64 % 2 != 0 {
            0
        } else {
            self.flags
        }
    }
}
fn encode_pair(dst: &mut [u8], f: &str, a: [u16; 3], b: [u16; 3]) {
    let [y, u, v] = a;
    let z = b[0];
    match f {
        "SDUY" => dst[..4].copy_from_slice(&[
            (u >> 2) as u8,
            (y >> 2) as u8,
            (v >> 2) as u8,
            (z >> 2) as u8,
        ]),
        "SDYU" => dst[..4].copy_from_slice(&[
            (y >> 2) as u8,
            (u >> 2) as u8,
            (z >> 2) as u8,
            (v >> 2) as u8,
        ]),
        "SDYV" => dst[..4].copy_from_slice(&[
            (y >> 2) as u8,
            (v >> 2) as u8,
            (z >> 2) as u8,
            (u >> 2) as u8,
        ]),
        "SD16" => {
            for (i, s) in [u, y, v, z].iter().enumerate() {
                dst[i * 2..i * 2 + 2].copy_from_slice(&s.to_le_bytes());
            }
        }
        "SDXU" => {
            let q = u as u64 | ((y as u64) << 10) | ((v as u64) << 20) | ((z as u64) << 30);
            dst[..8].copy_from_slice(&q.to_le_bytes());
        }
        "SDAR" => {
            for (i, [yy, cb, cr]) in [a, b].into_iter().enumerate() {
                let yy = (yy as f64 - 64.) * 255. / 876.;
                let cb = cb as f64 - 512.;
                let cr = cr as f64 - 512.;
                let rgb = [
                    yy + 1.402 * cr * 255. / 896.,
                    yy - (0.344136 * cb + 0.714136 * cr) * 255. / 896.,
                    yy + 1.772 * cb * 255. / 896.,
                ];
                dst[i * 4..i * 4 + 4].copy_from_slice(&[
                    255,
                    rgb[0].clamp(0., 255.) as u8,
                    rgb[1].clamp(0., 255.) as u8,
                    rgb[2].clamp(0., 255.) as u8,
                ]);
            }
        }
        _ => {}
    }
}
#[cfg(test)]
pub fn video(b: &mut [u8], l: Layout, frame: u64) -> Result<()> {
    let mut picture = crate::sapsan::Picture::new(l.width, l.height, 30, 1);
    video_picture(b, l, frame, &mut picture)
}
fn video_picture(
    b: &mut [u8],
    l: Layout,
    frame: u64,
    picture: &mut crate::sapsan::Picture,
) -> Result<()> {
    let f = code(l.fourcc);
    if !FORMATS.contains(&f.as_str()) {
        return Err(format!("unsupported format {f}"));
    }
    let w = l.width as usize;
    let h = l.height as usize;
    let stride = l.stride as usize;
    let row_bytes = if f == "SD10" {
        w.div_ceil(6) * 16
    } else {
        w * if ["SD16", "SDXU", "SDAR"].contains(&f.as_str()) {
            4
        } else {
            2
        }
    };
    if w < 256 || h < 8 || w % 2 != 0 || h % 2 != 0 || stride < row_bytes || stride * h > b.len() {
        return Err("invalid video layout".into());
    }
    let planar = picture.render(frame);
    let uvw = w / 2;
    let uvsize = w * h / 4;
    let pixel = |x: usize, y: usize| -> [u16; 3] {
        if y < 4 && x < 256 {
            return [if frame >> (x / 4) & 1 != 0 { 800 } else { 128 }, 512, 512];
        }
        let uv = w * h + y / 2 * uvw + x / 2;
        [
            planar[y * w + x] as u16 * 4,
            planar[uv] as u16 * 4,
            planar[uv + uvsize] as u16 * 4,
        ]
    };
    for y in 0..h {
        if y > 4
            && planar[y * w..(y + 1) * w] == planar[(y - 1) * w..y * w]
            && planar[w * h + y / 2 * uvw..w * h + (y / 2 + 1) * uvw]
                == planar[w * h + (y - 1) / 2 * uvw..w * h + ((y - 1) / 2 + 1) * uvw]
            && planar[w * h + uvsize + y / 2 * uvw..w * h + uvsize + (y / 2 + 1) * uvw]
                == planar
                    [w * h + uvsize + (y - 1) / 2 * uvw..w * h + uvsize + ((y - 1) / 2 + 1) * uvw]
        {
            b.copy_within((y - 1) * stride..y * stride, y * stride);
            continue;
        }
        let dst = &mut b[y * stride..(y + 1) * stride];
        dst.fill(0);
        if f == "SD10" {
            for x in (0..w).step_by(6) {
                let p: [[u16; 3]; 6] = std::array::from_fn(|n| pixel((x + n).min(w - 1), y));
                let v = [
                    p[0][1], p[0][0], p[0][2], p[1][0], p[2][1], p[2][0], p[2][2], p[3][0],
                    p[4][1], p[4][0], p[4][2], p[5][0],
                ];
                for n in 0..4 {
                    put32(
                        dst,
                        x / 6 * 16 + n * 4,
                        v[n * 3] as u32
                            | ((v[n * 3 + 1] as u32) << 10)
                            | ((v[n * 3 + 2] as u32) << 20),
                    );
                }
            }
        } else {
            let step = if ["SD16", "SDXU", "SDAR"].contains(&f.as_str()) {
                8
            } else {
                4
            };
            for x in (0..w).step_by(2) {
                encode_pair(&mut dst[x / 2 * step..], &f, pixel(x, y), pixel(x + 1, y));
            }
        }
    }
    Ok(())
}
pub fn luma(b: &[u8], l: Layout, x: u32, y: u32) -> Result<u16> {
    let row = b
        .get(y as usize * l.stride as usize..)
        .ok_or("missing video row")?;
    let f = code(l.fourcc);
    let x = x as usize;
    match f.as_str() {
        "SDUY" => Ok(*row.get(x * 2 + 1).ok_or("short UYVY")? as u16 * 4),
        "SDYU" | "SDYV" => Ok(*row.get(x * 2).ok_or("short YUYV")? as u16 * 4),
        "SD16" => {
            let off = x * 4 + 2;
            Ok(u16::from_le_bytes(
                row.get(off..off + 2)
                    .ok_or("short SD16")?
                    .try_into()
                    .unwrap(),
            ) & 1023)
        }
        "SDXU" => {
            let off = x / 2 * 8;
            let q = u64::from_le_bytes(
                row.get(off..off + 8)
                    .ok_or("short XU10")?
                    .try_into()
                    .unwrap(),
            );
            Ok(((q >> if x % 2 == 0 { 10 } else { 30 }) & 1023) as u16)
        }
        "SD10" => {
            let sample = [1, 3, 5, 7, 9, 11][x % 6];
            let q = get32(row, x / 6 * 16 + sample / 3 * 4)?;
            Ok(((q >> (sample % 3 * 10)) & 1023) as u16)
        }
        "SDAR" => {
            let off = x * 4;
            let rgb = row.get(off..off + 4).ok_or("short ARGB")?;
            Ok((64.
                + (0.299 * rgb[1] as f64 + 0.587 * rgb[2] as f64 + 0.114 * rgb[3] as f64) * 876.
                    / 255.)
                .round() as u16)
        }
        _ => Err(format!("cannot decode {f}")),
    }
}
pub fn marker(b: &[u8], l: Layout) -> Result<u64> {
    if l.width < 256 || l.height < 4 {
        return Err("picture too small for marker".into());
    }
    let mut v = 0;
    for bit in 0..64 {
        let y = luma(b, l, bit * 4 + 1, 1)?;
        if y > 464 {
            v |= 1 << bit;
        }
        if y.abs_diff(if y > 464 { 800 } else { 128 }) > 12 {
            return Err("video marker missing/corrupt".into());
        }
    }
    Ok(v)
}
pub fn check_video(b: &[u8], l: Layout, frame: u64, m: Mode) -> Result<()> {
    // The square and seven bars are deterministic across hosts. The top text
    // contains the transmitter hostname, so compare the frame-counter region
    // separately and sample the bars below the diagnostic plate.
    for y in [l.height / 4, l.height / 2, l.height * 3 / 4] {
        for n in 0..64 {
            let x = (n * l.width / 64).min(l.width - 1);
            let a = luma(b, l, x, y)?;
            let expected = crate::sapsan::expected_luma(
                x as usize,
                y as usize,
                l.width as usize,
                l.height as usize,
                frame,
                m.num,
                m.den,
            );
            if a.abs_diff(expected) > 16 {
                return Err(format!(
                    "video mismatch at {x},{y}: {a}, expected {expected}"
                ));
            }
        }
    }
    // The copied PTS band is in milliseconds; verify its decimal cells.
    let pts = (frame as u128 * 1000 * m.den as u128 / m.num.max(1) as u128) as u64;
    let digits = format!("{:016}", pts.min(9_999_999_999_999_999));
    for (cell, d) in digits.bytes().enumerate() {
        let x = ((cell * 2 + 1) * l.width as usize / 32) as u32;
        let y = l.height - l.height / 32;
        let got = luma(b, l, x, y)?;
        let mut expected = crate::sapsan::pts_luma::level_for_digit(d - b'0') as u16 * 4;
        if code(l.fourcc) == "SDAR" {
            expected = expected.clamp(64, 940);
        }
        if got.abs_diff(expected) > 16 {
            return Err("PTS luma band mismatch".into());
        }
    }
    Ok(())
}
pub fn tone(phase: u64, ch: usize) -> i32 {
    static TONES: std::sync::OnceLock<[[i32; 192]; 16]> = std::sync::OnceLock::new();
    let tones = TONES.get_or_init(|| {
        std::array::from_fn(|ch| {
            std::array::from_fn(|phase| {
                ((std::f64::consts::TAU * (4 + ch) as f64 * phase as f64 / 192.).sin() * 4194303.)
                    .round() as i32
            })
        })
    });
    tones[ch][(phase % 192) as usize]
}
pub fn encoded_word(phase: u64, ch: usize) -> u32 {
    const BURST: [u16; 8] = [0xf872, 0x4e1f, 1, 64, 0x0b77, 0, 0x0c40, 0x4040];
    (BURST
        .get((phase % 1536) as usize * 2 + ch)
        .copied()
        .unwrap_or(0) as u32)
        << 16
}
pub fn encoded_period(c: &Config) -> u64 {
    if c.eac3 {
        crate::eac3::period()
    } else {
        1536
    }
}
pub fn encoded_word_for(phase: u64, ch: usize, c: &Config) -> u32 {
    if c.eac3 {
        crate::eac3::word(phase, ch)
    } else {
        encoded_word(phase, ch)
    }
}
pub fn audio(b: &mut [u8], samples: usize, phase: u64, c: &Config) -> Result<()> {
    if (samples + c.pad as usize) * 64 > b.len() {
        return Err("audio buffer too small".into());
    }
    for i in 0..samples + c.pad as usize {
        for ch in 0..16 {
            let v = if i >= samples {
                0x40000000
            } else if ch >= c.channels as usize {
                0
            } else if c.nonpcm && ch < 2 {
                encoded_word_for(phase + i as u64, ch, c)
            } else {
                (tone(phase + i as u64, ch) as u32) << 8
            };
            put32(b, (i * 16 + ch) * 4, v);
        }
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub line: u16,
    pub did: u8,
    pub sdid: u8,
    pub flags: u8,
    pub data: Vec<u8>,
}
pub fn packets(b: &[u8]) -> Result<Vec<Packet>> {
    let mut off = 0;
    let mut out = vec![];
    while off < b.len() {
        let h = b.get(off..off + 8).ok_or("truncated ANC header")?;
        let count = h[6] as usize;
        let size = (8 + count * 2 + 3) & !3;
        if h.iter().all(|v| *v == 0) {
            if b[off..].iter().any(|v| *v != 0) {
                return Err("nonzero ANC after terminator".into());
            }
            break;
        }
        if count == 0 {
            return Err("empty ANC record".into());
        }
        let data = b
            .get(off + 8..off + 8 + count * 2)
            .ok_or("truncated ANC UDW")?;
        if off + size > b.len() {
            return Err("truncated ANC padding".into());
        }
        if h[7] & 4 != 0 {
            return Err("ANC checksum error flag".into());
        }
        let mut bytes = vec![];
        for v in data.chunks_exact(2) {
            let v = u16::from_le_bytes(v.try_into().unwrap());
            if v != word(v as u8) {
                return Err("ANC parity error".into());
            }
            bytes.push(v as u8);
        }
        out.push(Packet {
            line: u16::from_le_bytes([h[0], h[1]]),
            did: h[4],
            sdid: h[5],
            flags: h[7],
            data: bytes,
        });
        off += size;
    }
    Ok(out)
}
pub fn fixtures(frame: u64, mode: Mode) -> Vec<Packet> {
    let rate = (mode.fps() * if mode.interlaced != 0 { 2. } else { 1. }).round() as u64;
    let rate = rate.max(1);
    let tc = [
        frame % rate,
        frame / rate % 60,
        frame / (rate * 60) % 60,
        frame / (rate * 3600) % 24,
    ];
    let mut atc = vec![0; 16];
    for (i, t) in tc.into_iter().enumerate() {
        atc[i * 4] = ((t % 10) as u8) << 4;
        atc[i * 4 + 2] = ((t / 10) as u8) << 4;
    }
    let mut scte = vec![
        8,
        255,
        255,
        0,
        30,
        0,
        0,
        frame as u8,
        0,
        0,
        0,
        0,
        1,
        1,
        1,
        0,
        14,
        1,
        0,
        0,
        0,
        0,
        0,
        1,
        15,
        160,
        0,
        100,
        1,
        1,
        1,
    ];
    scte[18..22].copy_from_slice(&(frame as u32).to_be_bytes());
    vec![
        Packet {
            line: 9,
            did: 0x60,
            sdid: 0x60,
            flags: 1,
            data: atc,
        },
        Packet {
            line: if mode.total_lines == 625 || mode.total_lines == 525 {
                7
            } else {
                11
            },
            did: 0x41,
            sdid: 5,
            flags: 0,
            data: vec![
                64 | if frame / rate % 2 == 0 { 0 } else { 4 },
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        },
        Packet {
            line: 12,
            did: 0x41,
            sdid: 7,
            // ST 2010 section 6 requires SCTE-104 in the Y stream for HD.
            flags: 0,
            data: scte,
        },
        Packet {
            line: 13,
            did: 0x43,
            sdid: 2,
            flags: 0,
            data: crate::teletext::op47(frame),
        },
        Packet {
            line: 14,
            did: 0x61,
            sdid: 1,
            flags: 0,
            data: vec![0x96, 0x69, 0, 0],
        },
        Packet {
            line: 15,
            did: 0x5f,
            sdid: 0xfa,
            flags: 0,
            data: frame.to_le_bytes().to_vec(),
        },
    ]
}
pub fn anc(b: &mut [u8], ps: &[Packet]) -> Result<usize> {
    let mut off = 0;
    for p in ps {
        if p.data.len() > 255 {
            return Err("ANC payload exceeds 255".into());
        }
        let n = (8 + p.data.len() * 2 + 3) & !3;
        let dst = b.get_mut(off..off + n).ok_or("ANC buffer too small")?;
        dst.fill(0);
        dst[..2].copy_from_slice(&p.line.to_le_bytes());
        dst[4] = p.did;
        dst[5] = p.sdid;
        dst[6] = p.data.len() as u8;
        dst[7] = p.flags;
        for (i, v) in p.data.iter().enumerate() {
            dst[8 + i * 2..10 + i * 2].copy_from_slice(&word(*v).to_le_bytes());
        }
        off += n;
    }
    Ok(off)
}
pub fn vbi(b: &mut [u8], lines: u32, frame: u64) -> Result<usize> {
    let rows = match lines {
        625 => 34,
        525 => 24,
        _ => return Ok(0),
    };
    let size = rows * 1440;
    let dst = b.get_mut(..size).ok_or("VBI buffer too small")?;
    dst.fill(0);
    if lines == 625 {
        for (line, header) in [(10, true), (11, false), (323, true), (324, false)] {
            let r = crate::teletext::row(lines, line).unwrap();
            crate::teletext::render(
                &mut dst[r * 1440..(r + 1) * 1440],
                &crate::teletext::packet(frame, header),
            )?;
        }
    } else {
        for line in [21, 284] {
            let row = crate::teletext::row(lines, line).unwrap();
            for (i, s) in dst[row * 1440..(row + 1) * 1440]
                .chunks_exact_mut(2)
                .enumerate()
            {
                let v = if (i / 12 + frame as usize) % 2 == 0 {
                    64u16
                } else {
                    800
                };
                s.copy_from_slice(&v.to_le_bytes());
            }
        }
    }

    Ok(size)
}
pub struct Generator {
    pub frame: u64,
    pub phase: u64,
    pub remainder: u64,
    pub config: Config,
    picture: Option<crate::sapsan::Picture>,
}
impl Generator {
    pub fn new(config: Config) -> Self {
        Self {
            frame: 0,
            phase: 0,
            remainder: 0,
            config,
            picture: None,
        }
    }
    pub fn fill(&mut self, p: [&mut [u8]; 5], l: Layout, m: Mode) -> Result<[u32; 5]> {
        if m.num == 0 || m.den == 0 {
            return Err("invalid frame rate".into());
        }
        if self.picture.is_none() {
            self.picture = Some(crate::sapsan::Picture::new(l.width, l.height, m.num, m.den));
        }
        video_picture(p[0], l, self.frame, self.picture.as_mut().unwrap())?;
        self.remainder += 48000 * m.den;
        let samples = (self.remainder / m.num) as usize;
        self.remainder %= m.num;
        audio(p[1], samples, self.phase, &self.config)?;
        let al = if self.config.anc {
            anc(p[2], &fixtures(self.frame, m))?
        } else {
            0
        };
        let meta = p[3].get_mut(..128).ok_or("metadata buffer too short")?;
        meta.fill(0);
        if !self.config.no_meta {
            put32(meta, 0, META_MAGIC);
            put32(meta, 4, 4);
            put32(meta, 8, self.config.flags_at(self.frame));
            put32(meta, 24, (1 << self.config.channels) - 1);
            put32(
                meta,
                28,
                if m.interlaced != 0 {
                    (samples as u32).div_ceil(2)
                } else {
                    samples as u32
                },
            );
            put32(
                meta,
                32,
                if m.interlaced != 0 {
                    samples as u32 / 2
                } else {
                    0
                },
            );
            put32(meta, 36, 48000);
            put32(meta, 40, if self.config.nonpcm { 3 } else { 0 });
        }
        let vl = if self.config.vbi {
            vbi(p[4], m.total_lines, self.frame)?
        } else {
            0
        };
        self.phase += samples as u64;
        self.frame += 1;
        Ok([
            l.stride * l.height,
            ((samples + self.config.pad as usize) * 64) as u32,
            al as u32,
            128,
            vl as u32,
        ])
    }
}
