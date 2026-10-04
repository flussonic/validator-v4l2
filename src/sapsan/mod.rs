// SPDX-License-Identifier: MIT
// Drawing routines adapted from the Flussonic Sapsan synthetic source.
pub mod pts_luma;
mod text_overlay;
use text_overlay::TextOverlay;
#[derive(Clone, Copy)]
struct FrameRate {
    numerator: u32,
    denominator: u32,
}
struct FrameSize {
    width: u32,
    height: u32,
}
struct SourceIdentity {
    host: String,
}
impl SourceIdentity {
    fn stream(&self) -> &str {
        "validator-v4l2"
    }
    fn host(&self) -> &str {
        &self.host
    }
}
const MOVING_SQUARE_SIDE_DIVISOR: usize = 8;
const MOVING_SQUARE_SIDE_TRAVEL_MS: f64 = 1_500.0;
const PTS_BAND_HEIGHT_DIVISOR: usize = 16;
const MIN_PTS_BAND_HEIGHT: usize = 8;
const COLOR_BARS: [(u8, u8, u8); 7] = [
    (235, 128, 128), // white
    (210, 16, 146),  // yellow
    (170, 166, 16),  // cyan
    (145, 54, 34),   // green
    (106, 202, 222), // magenta
    (81, 90, 240),   // red
    (41, 240, 110),  // blue
];
const SQUARE_COLOR: (u8, u8, u8) = (180, 128, 128);

fn moving_square_side(height: usize) -> usize {
    (height / MOVING_SQUARE_SIDE_DIVISOR).max(1)
}

fn moving_square_center(
    frame_index: u64,
    rate: FrameRate,
    width: usize,
    height: usize,
) -> (usize, usize) {
    if width == 0 || height == 0 {
        return (0, 0);
    }

    let side = moving_square_side(height).min(width).min(height).max(1);
    let safe_height = height.saturating_sub(pts_band_height(height));
    let (min_x, max_x) = center_axis_bounds(width, side);
    let (min_y, max_y) = center_axis_bounds(safe_height.max(1), side.min(safe_height.max(1)));

    let elapsed_ms = frame_index as f64 * 1000.0 * f64::from(rate.denominator)
        / f64::from(rate.numerator.max(1));
    let speed_px_per_ms = side as f64 / MOVING_SQUARE_SIDE_TRAVEL_MS;
    let component_speed = speed_px_per_ms * std::f64::consts::FRAC_1_SQRT_2;

    let x = reflect_position(min_x as f64, max_x as f64, elapsed_ms * component_speed);
    let y = reflect_position(min_y as f64, max_y as f64, elapsed_ms * component_speed);
    (x.round() as usize, y.round() as usize)
}

fn center_axis_bounds(limit: usize, side: usize) -> (usize, usize) {
    if limit == 0 {
        return (0, 0);
    }
    let side = side.min(limit).max(1);
    let half = side / 2;
    let tail = side - half;
    let min_center = half;
    let max_center = limit.saturating_sub(tail);
    if max_center < min_center {
        let center = (limit - 1) / 2;
        (center, center)
    } else {
        (min_center, max_center)
    }
}

fn reflect_position(min: f64, max: f64, distance: f64) -> f64 {
    if max <= min {
        return min;
    }
    let span = max - min;
    let period = span * 2.0;
    let phase = distance.rem_euclid(period);
    if phase <= span {
        min + phase
    } else {
        max - (phase - span)
    }
}

fn draw_color_bars_yuv420p(buffer: &mut [u8], width: usize, height: usize) {
    let y_size = width * height;
    let uv_w = width / 2;
    let uv_h = height / 2;

    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let bar = (x * 7 / width).min(6);
            buffer[row + x] = COLOR_BARS[bar].0;
        }
    }

    for y in 0..uv_h {
        for x in 0..uv_w {
            let bar = (x * 7 / uv_w).min(6);
            let index = y_size + y * uv_w + x;
            buffer[index] = COLOR_BARS[bar].1;
            buffer[y_size + uv_w * uv_h + y * uv_w + x] = COLOR_BARS[bar].2;
        }
    }
}

fn neutralize_chroma_in_pts_band(buffer: &mut [u8], width: usize, height: usize) {
    let band_h = pts_band_height(height);
    let band_y0 = height.saturating_sub(band_h);
    let y_size = width * height;
    let uv_w = width / 2;
    let uv_h = height / 2;
    let uv_band_y0 = band_y0 / 2;

    for y in uv_band_y0..uv_h {
        for x in 0..uv_w {
            let index = y_size + y * uv_w + x;
            buffer[index] = 128;
            buffer[y_size + uv_w * uv_h + y * uv_w + x] = 128;
        }
    }
}

fn draw_moving_square_yuv420p(
    buffer: &mut [u8],
    width: usize,
    height: usize,
    center: (usize, usize),
    side: usize,
) {
    if width == 0 || height == 0 {
        return;
    }
    let side = side.min(width).min(height).max(1);
    let half = side / 2;

    let x0 = center.0.saturating_sub(half).min(width - 1);
    let y0 = center.1.saturating_sub(half).min(height - 1);
    let x1 = x0.saturating_add(side).min(width);
    let y1 = y0.saturating_add(side).min(height);

    for y in y0..y1 {
        let row = y * width;
        for x in x0..x1 {
            buffer[row + x] = SQUARE_COLOR.0;
        }
    }

    let y_size = width * height;
    let uv_w = width / 2;
    let uv_h = height / 2;
    let uv_x0 = (x0 / 2).min(uv_w);
    let uv_x1 = x1.div_ceil(2).min(uv_w);
    let uv_y0 = (y0 / 2).min(uv_h);
    let uv_y1 = y1.div_ceil(2).min(uv_h);
    for y in uv_y0..uv_y1 {
        for x in uv_x0..uv_x1 {
            let index = y_size + y * uv_w + x;
            buffer[index] = SQUARE_COLOR.1;
            buffer[y_size + uv_w * uv_h + y * uv_w + x] = SQUARE_COLOR.2;
        }
    }
}

fn pts_band_height(height: usize) -> usize {
    (height / PTS_BAND_HEIGHT_DIVISOR).max(MIN_PTS_BAND_HEIGHT)
}

pub struct Picture {
    width: usize,
    height: usize,
    rate: FrameRate,
    base: Vec<u8>,
    image: Vec<u8>,
    overlay: Option<TextOverlay>,
}
impl Picture {
    pub fn new(width: u32, height: u32, num: u64, den: u64) -> Self {
        let rate = frame_rate(num, den);
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .unwrap_or_else(|_| "localhost".into())
            .trim()
            .to_string();
        let overlay =
            TextOverlay::new(&SourceIdentity { host }, &FrameSize { width, height }, rate);
        let (width, height) = (width as usize, height as usize);
        let mut base = vec![0; width * height * 3 / 2];
        draw_color_bars_yuv420p(&mut base, width, height);
        Self {
            width,
            height,
            rate,
            image: base.clone(),
            base,
            overlay,
        }
    }
    pub fn image(&self) -> &[u8] {
        &self.image
    }
    pub fn frame_rect(&self) -> Option<(usize, usize, usize, usize)> {
        self.overlay.as_ref().and_then(TextOverlay::frame_rect)
    }
    pub fn render(&mut self, frame: u64) -> &[u8] {
        self.image.copy_from_slice(&self.base);
        draw_moving_square_yuv420p(
            &mut self.image,
            self.width,
            self.height,
            moving_square_center(frame, self.rate, self.width, self.height),
            moving_square_side(self.height),
        );
        let pts_ms = (frame as u128 * 1000 * self.rate.denominator as u128
            / self.rate.numerator.max(1) as u128) as u64;
        pts_luma::encode_pts_into_luma(&mut self.image, self.width, self.height, pts_ms);
        neutralize_chroma_in_pts_band(&mut self.image, self.width, self.height);
        if let Some(o) = self.overlay.as_mut() {
            o.draw(&mut self.image, pts_ms, frame);
        }
        &self.image
    }
}
pub fn expected_luma(
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    frame: u64,
    num: u64,
    den: u64,
) -> u16 {
    let center = moving_square_center(frame, frame_rate(num, den), w, h);
    let side = moving_square_side(h);
    let x0 = center.0.saturating_sub(side / 2);
    let y0 = center.1.saturating_sub(side / 2);
    if x >= x0 && x < x0 + side && y >= y0 && y < y0 + side {
        SQUARE_COLOR.0 as u16 * 4
    } else {
        COLOR_BARS[(x * 7 / w).min(6)].0 as u16 * 4
    }
}

fn frame_rate(num: u64, den: u64) -> FrameRate {
    let mut a = num;
    let mut b = den;
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    FrameRate {
        numerator: (num / a.max(1)) as u32,
        denominator: (den / a.max(1)) as u32,
    }
}
