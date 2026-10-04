// SPDX-License-Identifier: MIT
// Synthetic six-channel tones; see fixtures/README.md for reproducible encoding.
pub const DATA: &[u8] = include_bytes!("../fixtures/eac3-5.1-48k.eac3");
pub const SAMPLES: u64 = 1536;

pub fn frames() -> &'static Vec<&'static [u8]> {
    static FRAMES: std::sync::OnceLock<Vec<&'static [u8]>> = std::sync::OnceLock::new();
    FRAMES.get_or_init(|| {
        let mut out = Vec::new();
        let mut rest = DATA;
        while !rest.is_empty() {
            assert!(rest.len() >= 6 && rest[..2] == [0x0b, 0x77]);
            let size = ((((rest[2] as usize & 7) << 8) | rest[3] as usize) + 1) * 2;
            assert!(size <= rest.len() && size + 8 <= (SAMPLES as usize - 2) * 4);
            // Independent substream, 48 kHz, six blocks, bsid 16.
            assert_eq!(rest[2] >> 6, 0);
            assert_eq!(rest[4] >> 4, 3);
            assert_eq!(rest[5] >> 3, 16);
            out.push(&rest[..size]);
            rest = &rest[size..];
        }
        out
    })
}

pub fn period() -> u64 {
    frames().len() as u64 * SAMPLES
}

pub fn word(phase: u64, channel: usize) -> u32 {
    let phase = phase % period();
    let frame = frames()[(phase / SAMPLES) as usize];
    let index = (phase % SAMPLES) as usize * 2 + channel;
    let value = match index {
        0 => 0xf872,
        1 => 0x4e1f,
        2 => 16,                       // SMPTE ST 340 E-AC-3, not IEC 61937 type 21.
        3 => (frame.len() * 8) as u16, // ST 337 Pd counts bits.
        _ => frame
            .get((index - 4) * 2..(index - 3) * 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .unwrap_or(0),
    };
    u32::from(value) << 16
}
