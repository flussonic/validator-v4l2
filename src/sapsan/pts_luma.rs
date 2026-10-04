// SPDX-License-Identifier: MIT
// Adapted from the Flussonic Sapsan synthetic source.
pub const PTS_DIGITS: usize = 16;
pub const PTS_CELLS: usize = 16;

const MAX_PTS_TICKS: u64 = 9_999_999_999_999_999;

/// Draws PTS as a 16-digit luma bar along the bottom of a densely packed Y plane (`stride == width`).
pub fn encode_pts_into_luma(y_plane: &mut [u8], width: usize, height: usize, pts_ticks: u64) {
    let band_h = band_height(height);
    let band_y0 = height.saturating_sub(band_h);
    let digits = pts_digits(pts_ticks);

    for (cell, &ascii_digit) in digits.iter().enumerate() {
        let digit = ascii_digit - b'0';
        let level = level_for_digit(digit);
        let x0 = cell * width / PTS_CELLS;
        let x1 = (cell + 1) * width / PTS_CELLS;

        for y in band_y0..height {
            let row = y * width;
            for x in x0..x1 {
                y_plane[row + x] = level;
            }
        }
    }
}

/// Reads PTS from the bottom luma bar. `stride` is the Y row pitch (`width` for dense buffers).
#[cfg(test)]
pub fn decode_pts_from_luma(y_plane: &[u8], stride: usize, width: usize, height: usize) -> u64 {
    let band_h = band_height(height);
    let band_y0 = height.saturating_sub(band_h);
    let sample_y0 = band_y0 + band_h / 4;
    let sample_y1 = band_y0 + band_h - band_h / 4;

    let mut value = 0u64;
    for cell in 0..PTS_CELLS {
        let x0 = cell * width / PTS_CELLS;
        let x1 = (cell + 1) * width / PTS_CELLS;
        let cell_w = x1 - x0;
        let sample_x0 = x0 + cell_w / 4;
        let sample_x1 = x1 - cell_w / 4;

        let mut sum = 0u64;
        let mut count = 0u64;
        for y in sample_y0..sample_y1 {
            let row = y * stride;
            for x in sample_x0..sample_x1 {
                sum += u64::from(y_plane[row + x]);
                count += 1;
            }
        }

        let avg = if count == 0 {
            0
        } else {
            sum.checked_add(count / 2)
                .and_then(|rounded| rounded.checked_div(count))
                .unwrap_or(0)
        };
        let digit = digit_for_level(avg as u8);
        value = value * 10 + u64::from(digit);
    }

    value
}

pub(crate) fn level_for_digit(d: u8) -> u8 {
    debug_assert!(d <= 9);
    ((u16::from(d) * 255 + 4) / 9) as u8
}

#[cfg(test)]
pub(crate) fn digit_for_level(luma: u8) -> u8 {
    (((u16::from(luma) * 9) + 127) / 255).min(9) as u8
}

fn band_height(height: usize) -> usize {
    (height / 16).max(8)
}

fn pts_digits(pts_ticks: u64) -> [u8; PTS_DIGITS] {
    let mut digits = [b'0'; PTS_DIGITS];
    let mut value = pts_ticks.min(MAX_PTS_TICKS);
    for slot in (0..PTS_DIGITS).rev() {
        digits[slot] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    digits
}
