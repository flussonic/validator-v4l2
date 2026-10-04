// SPDX-License-Identifier: MIT
// Independent EN 300 706 encoder/slicer for 13.5 MHz VBI luma samples.
use crate::Result;
fn hamming(n: u8) -> u8 {
    let [a, b, c, d] = std::array::from_fn::<_, 4, _>(|i| (n >> i) & 1);
    let p = 1 ^ a ^ c ^ d;
    let q = 1 ^ a ^ b ^ d;
    let r = 1 ^ a ^ b ^ c;
    let s = 1 ^ a ^ b ^ c ^ d ^ p ^ q ^ r;
    p | (a << 1) | (q << 2) | (b << 3) | (r << 4) | (c << 5) | (s << 6) | (d << 7)
}
fn odd(n: u8) -> u8 {
    let n = n & 127;
    n | ((((n.count_ones() & 1) ^ 1) as u8) << 7)
}
pub fn packet(frame: u64, header: bool) -> [u8; 42] {
    let mut p = [odd(b' '); 42];
    let row = if header { 0 } else { 1 };
    let address = (row << 3) | 1;
    p[0] = hamming(address & 15);
    p[1] = hamming(address >> 4);
    let offset = if header {
        for b in &mut p[2..10] {
            *b = hamming(0);
        }
        10
    } else {
        2
    };
    let text = format!("VALIDATOR PAGE 100 FRAME {frame:09}");
    for (b, c) in p[offset..].iter_mut().zip(text.bytes()) {
        *b = odd(c);
    }
    p
}
pub fn render(dst: &mut [u8], packet: &[u8; 42]) -> Result<()> {
    if dst.len() != 1440 {
        return Err("teletext row must hold 720 samples".into());
    }
    let mut payload = [0; 45];
    payload[..3].copy_from_slice(&[0x55, 0x55, 0x27]);
    payload[3..].copy_from_slice(packet);
    for (sample, word) in dst.chunks_exact_mut(2).enumerate() {
        let bit = sample.checked_sub(16).map(|s| s * 444 / 864);
        let high = bit.is_some_and(|bit| bit < 360 && payload[bit / 8] & (1 << (bit % 8)) != 0);
        word.copy_from_slice(&if high { 642u16 } else { 64 }.to_le_bytes());
    }
    Ok(())
}
pub fn slice(src: &[u8]) -> Result<[u8; 42]> {
    if src.len() != 1440 {
        return Err("invalid teletext row size".into());
    }
    let y: Vec<u16> = src
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes(b.try_into().unwrap()) & 1023)
        .collect();
    let low = *y[..128].iter().min().unwrap();
    let high = *y[..128].iter().max().unwrap();
    if high - low < 128 {
        return Err("teletext clock run-in absent".into());
    }
    let threshold = (low + high) / 2;
    // Search sub-sample phase; accept only the complete standard sync prefix.
    for start_quarters in 0..512 {
        let mut bytes = [0u8; 45];
        let mut complete = true;
        for bit in 0..360 {
            let pos = start_quarters as f64 / 4. + (bit as f64 + 0.5) * 864. / 444.;
            let index = pos.ceil() as usize;
            if index >= 720 {
                complete = false;
                break;
            }
            if y[index] > threshold {
                bytes[bit / 8] |= 1 << (bit % 8);
            }
        }
        if complete && bytes[..3] == [0x55, 0x55, 0x27] {
            let mut out = [0u8; 42];
            out.copy_from_slice(&bytes[3..]);
            if !out[2..].iter().all(|b| b.count_ones() % 2 == 1) {
                continue;
            }
            if ![out[0], out[1]]
                .iter()
                .all(|b| (0..16).any(|n| hamming(n) == *b))
            {
                continue;
            }
            return Ok(out);
        }
    }
    Err("teletext framing/parity mismatch".into())
}
pub fn op47(frame: u64) -> Vec<u8> {
    let packet = packet(frame, false);
    let mut out = vec![0x51, 0x15, 58, 2, 0x8a, 0, 0, 0, 0, 0x55, 0x55, 0x27];
    out.extend_from_slice(&packet);
    out.extend_from_slice(&[0x74, (frame >> 8) as u8, frame as u8, 0]);
    let checksum = out[..out.len() - 1]
        .iter()
        .fold(0u8, |s, b| s.wrapping_add(*b));
    *out.last_mut().unwrap() = checksum.wrapping_neg();
    out
}
pub fn row(lines: u32, line: u32) -> Option<usize> {
    match lines {
        625 if (6..=22).contains(&line) => Some((line - 6) as usize),
        625 if (319..=335).contains(&line) => Some((17 + line - 319) as usize),
        525 if (10..=21).contains(&line) => Some((line - 10) as usize),
        525 if (273..=284).contains(&line) => Some((12 + line - 273) as usize),
        _ => None,
    }
}
pub fn check_vbi(src: &[u8], lines: u32, frame: u64) -> Result<()> {
    if lines != 625 {
        return Ok(());
    }
    for (line, header) in [(10, true), (11, false), (323, true), (324, false)] {
        let row = row(lines, line).unwrap();
        let bytes = src
            .get(row * 1440..(row + 1) * 1440)
            .ok_or("missing teletext VBI row")?;
        if slice(bytes)? != packet(frame, header) {
            return Err(format!("teletext payload mismatch on line {line}"));
        }
    }
    Ok(())
}
