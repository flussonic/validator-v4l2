// SPDX-License-Identifier: MIT
// Sapsan text renderer; UTC formatting and source identity adapted for std-only builds.
//! Плашка с диагностическим текстом поверх синтетического кадра: время, имя
//! стрима, хостнейм генерации, разрешение с частотой и номер кадра (#63892).
//!
//! Шрифт нарисован здесь же, а не взят готовым, и это осознанный выбор.
//! Растеризатор TrueType (`fontdue`, `ab_glyph`) сам по себе безобиден, но
//! требует файла шрифта, а любой пригодный (DejaVu, Liberation, Noto, Terminus)
//! идёт под OFL или Bitstream Vera: это обязывает возить текст лицензии в
//! поставке, добавляет в бинарь сотни килобайт и кэш глифов на кадр. Здесь
//! нужен ASCII шириной в пять точек, поэтому таблица ниже — 95 глифов, около
//! килобайта данных, ноль зависимостей и ноль лицензионных обязательств.
//!
//! Клетка 5x8: семь строк занимает заглавная буква, восьмая оставлена под
//! выносные элементы `g j p q y`.
//!
//! Данные шрифта строго однобитные, а полутон появляется только при отрисовке:
//! маска глифа один раз на масштаб размывается ядром шириной в одну исходную
//! точку, и дальше кадр за кадром идёт только смешивание. Замер на x264
//! показал, что мягкий край по битам нейтрален (разница с рубленым — три
//! процента в обе стороны), поэтому выбран он по внешнему виду: без него
//! диагонали `2 x m @` идут лесенкой. Подложка, наоборот, непрозрачная
//! намеренно: полупрозрачная стоит на треть больше битов и на голодном
//! битрейте идёт пятнами, потому что под ней остаётся живая картинка.

use super::SourceIdentity;
use super::{FrameRate, FrameSize};
use std::fmt::Write as _;

/// Ширина знакоместа в точках исходного битмапа.
const CELL_W: usize = 5;
/// Высота знакоместа вместе со строкой выносных элементов.
const CELL_H: usize = 8;
/// Шаг между началами соседних букв: знакоместо плюс просвет.
const ADVANCE: usize = CELL_W + 1;
/// Шаг между началами соседних строк: знакоместо плюс просвет.
const LINE_PITCH: usize = CELL_H + 1;

const FIRST_GLYPH: u8 = b' ';
const LAST_GLYPH: u8 = b'~';
const GLYPH_COUNT: usize = (LAST_GLYPH - FIRST_GLYPH + 1) as usize;
/// Чем подменяется всё, чего нет в таблице: не-ASCII и управляющие символы.
const FALLBACK_GLYPH: u8 = b'?';

/// Яркость подложки и глифа: пределы студийного диапазона.
const PLATE_LUMA: u8 = 16;
const GLYPH_LUMA: u8 = 235;
const NEUTRAL_CHROMA: u8 = 128;

/// Плашка обязана уместиться в верхнюю пятую часть кадра: ниже идёт картинка,
/// а по нижнему краю живёт машиночитаемая полоса PTS.
const MAX_PLATE_HEIGHT_DIVISOR: usize = 5;
/// Отступ плашки от края кадра и текста от края плашки, в знакоместах.
const OUTER_PAD_CELLS: usize = 4;
const INNER_PAD_CELLS: usize = 2;
/// Знакомест под номер кадра: на 25 fps этого хватает больше чем на год.
const FRAME_COUNTER_DIGITS: usize = 9;

/// Ширина строки времени в знакоместах: формат фиксирован, ширина тоже.
const TIME_LINE_WIDTH: usize = "2026-08-24 12:34:56.789 UTC".len();

/// Строки в плашке идут в порядке убывания важности: кадру может не хватить
/// высоты на все, и тогда отбрасывается хвост. Меняющиеся строки — время и
/// номер кадра — переписываются каждый кадр, остальные посчитаны один раз.
const LINE_TIME: usize = 0;
const LINE_FRAME: usize = 3;

/// Битмап 5x8 на каждый печатный символ ASCII, по порядку кодов от `0x20`.
/// Строки глифа разделены `|`, точка обозначена `#`.
const FONT: [(char, &str); GLYPH_COUNT] = [
    (' ', ".....|.....|.....|.....|.....|.....|.....|....."),
    ('!', "..#..|..#..|..#..|..#..|..#..|.....|..#..|....."),
    ('"', ".#.#.|.#.#.|.....|.....|.....|.....|.....|....."),
    ('#', ".#.#.|.#.#.|#####|.#.#.|#####|.#.#.|.#.#.|....."),
    ('$', "..#..|.####|#.#..|.###.|..#.#|####.|..#..|....."),
    ('%', "##...|##..#|...#.|..#..|.#...|#..##|...##|....."),
    ('&', ".##..|#..#.|#.#..|.#...|#.#.#|#..#.|.##.#|....."),
    ('\'', "..#..|..#..|.....|.....|.....|.....|.....|....."),
    ('(', "...#.|..#..|.#...|.#...|.#...|..#..|...#.|....."),
    (')', ".#...|..#..|...#.|...#.|...#.|..#..|.#...|....."),
    ('*', ".....|#.#.#|.###.|#####|.###.|#.#.#|.....|....."),
    ('+', ".....|..#..|..#..|#####|..#..|..#..|.....|....."),
    (',', ".....|.....|.....|.....|.....|..##.|..#..|.#..."),
    ('-', ".....|.....|.....|#####|.....|.....|.....|....."),
    ('.', ".....|.....|.....|.....|.....|.##..|.##..|....."),
    ('/', "....#|....#|...#.|..#..|.#...|#....|#....|....."),
    ('0', ".###.|#...#|#..##|#.#.#|##..#|#...#|.###.|....."),
    ('1', "..#..|.##..|..#..|..#..|..#..|..#..|.###.|....."),
    ('2', ".###.|#...#|....#|...#.|..#..|.#...|#####|....."),
    ('3', "#####|...#.|..#..|...#.|....#|#...#|.###.|....."),
    ('4', "...#.|..##.|.#.#.|#..#.|#####|...#.|...#.|....."),
    ('5', "#####|#....|####.|....#|....#|#...#|.###.|....."),
    ('6', "..##.|.#...|#....|####.|#...#|#...#|.###.|....."),
    ('7', "#####|....#|...#.|..#..|.#...|.#...|.#...|....."),
    ('8', ".###.|#...#|#...#|.###.|#...#|#...#|.###.|....."),
    ('9', ".###.|#...#|#...#|.####|....#|...#.|.##..|....."),
    (':', ".....|.##..|.##..|.....|.##..|.##..|.....|....."),
    (';', ".....|.##..|.##..|.....|.##..|..#..|.#...|....."),
    ('<', "...#.|..#..|.#...|#....|.#...|..#..|...#.|....."),
    ('=', ".....|.....|#####|.....|#####|.....|.....|....."),
    ('>', ".#...|..#..|...#.|....#|...#.|..#..|.#...|....."),
    ('?', ".###.|#...#|....#|...#.|..#..|.....|..#..|....."),
    ('@', ".###.|#...#|#.###|#.#.#|#.###|#....|.###.|....."),
    ('A', ".###.|#...#|#...#|#####|#...#|#...#|#...#|....."),
    ('B', "####.|#...#|#...#|####.|#...#|#...#|####.|....."),
    ('C', ".###.|#...#|#....|#....|#....|#...#|.###.|....."),
    ('D', "###..|#..#.|#...#|#...#|#...#|#..#.|###..|....."),
    ('E', "#####|#....|#....|####.|#....|#....|#####|....."),
    ('F', "#####|#....|#....|####.|#....|#....|#....|....."),
    ('G', ".###.|#...#|#....|#.###|#...#|#...#|.####|....."),
    ('H', "#...#|#...#|#...#|#####|#...#|#...#|#...#|....."),
    ('I', ".###.|..#..|..#..|..#..|..#..|..#..|.###.|....."),
    ('J', "..###|...#.|...#.|...#.|...#.|#..#.|.##..|....."),
    ('K', "#...#|#..#.|#.#..|##...|#.#..|#..#.|#...#|....."),
    ('L', "#....|#....|#....|#....|#....|#....|#####|....."),
    ('M', "#...#|##.##|#.#.#|#...#|#...#|#...#|#...#|....."),
    ('N', "#...#|##..#|#.#.#|#..##|#...#|#...#|#...#|....."),
    ('O', ".###.|#...#|#...#|#...#|#...#|#...#|.###.|....."),
    ('P', "####.|#...#|#...#|####.|#....|#....|#....|....."),
    ('Q', ".###.|#...#|#...#|#...#|#.#.#|#..#.|.##.#|....."),
    ('R', "####.|#...#|#...#|####.|#.#..|#..#.|#...#|....."),
    ('S', ".####|#....|#....|.###.|....#|....#|####.|....."),
    ('T', "#####|..#..|..#..|..#..|..#..|..#..|..#..|....."),
    ('U', "#...#|#...#|#...#|#...#|#...#|#...#|.###.|....."),
    ('V', "#...#|#...#|#...#|#...#|#...#|.#.#.|..#..|....."),
    ('W', "#...#|#...#|#...#|#.#.#|#.#.#|##.##|#...#|....."),
    ('X', "#...#|#...#|.#.#.|..#..|.#.#.|#...#|#...#|....."),
    ('Y', "#...#|#...#|.#.#.|..#..|..#..|..#..|..#..|....."),
    ('Z', "#####|....#|...#.|..#..|.#...|#....|#####|....."),
    ('[', "..###|..#..|..#..|..#..|..#..|..#..|..###|....."),
    ('\\', "#....|#....|.#...|..#..|...#.|....#|....#|....."),
    (']', "###..|..#..|..#..|..#..|..#..|..#..|###..|....."),
    ('^', "..#..|.#.#.|#...#|.....|.....|.....|.....|....."),
    ('_', ".....|.....|.....|.....|.....|.....|.....|#####"),
    ('`', ".#...|..#..|.....|.....|.....|.....|.....|....."),
    ('a', ".....|.....|.###.|....#|.####|#...#|.####|....."),
    ('b', "#....|#....|####.|#...#|#...#|#...#|####.|....."),
    ('c', ".....|.....|.###.|#...#|#....|#...#|.###.|....."),
    ('d', "....#|....#|.####|#...#|#...#|#...#|.####|....."),
    ('e', ".....|.....|.###.|#...#|#####|#....|.###.|....."),
    ('f', "..##.|.#...|.#...|####.|.#...|.#...|.#...|....."),
    ('g', ".....|.....|.####|#...#|#...#|.####|....#|.###."),
    ('h', "#....|#....|####.|#...#|#...#|#...#|#...#|....."),
    ('i', "..#..|.....|.##..|..#..|..#..|..#..|.###.|....."),
    ('j', "...#.|.....|..##.|...#.|...#.|...#.|#..#.|.##.."),
    ('k', "#....|#....|#..#.|#.#..|##...|#.#..|#..#.|....."),
    ('l', ".##..|..#..|..#..|..#..|..#..|..#..|.###.|....."),
    ('m', ".....|.....|##.#.|#.#.#|#.#.#|#.#.#|#.#.#|....."),
    ('n', ".....|.....|####.|#...#|#...#|#...#|#...#|....."),
    ('o', ".....|.....|.###.|#...#|#...#|#...#|.###.|....."),
    ('p', ".....|.....|####.|#...#|#...#|####.|#....|#...."),
    ('q', ".....|.....|.####|#...#|#...#|.####|....#|....#"),
    ('r', ".....|.....|#.##.|##..#|#....|#....|#....|....."),
    ('s', ".....|.....|.####|#....|.###.|....#|####.|....."),
    ('t', ".#...|.#...|####.|.#...|.#...|.#..#|..##.|....."),
    ('u', ".....|.....|#...#|#...#|#...#|#..##|.##.#|....."),
    ('v', ".....|.....|#...#|#...#|#...#|.#.#.|..#..|....."),
    ('w', ".....|.....|#...#|#.#.#|#.#.#|#.#.#|.#.#.|....."),
    ('x', ".....|.....|#...#|.#.#.|..#..|.#.#.|#...#|....."),
    ('y', ".....|.....|#...#|#...#|#...#|.####|....#|.###."),
    ('z', ".....|.....|#####|...#.|..#..|.#...|#####|....."),
    ('{', "...##|..#..|..#..|.##..|..#..|..#..|...##|....."),
    ('|', "..#..|..#..|..#..|..#..|..#..|..#..|..#..|....."),
    ('}', "##...|..#..|..#..|..##.|..#..|..#..|##...|....."),
    ('~', ".....|.....|.##.#|#..##|.....|.....|.....|....."),
];

/// Маски всех глифов, растянутые под один масштаб. Полутон края посчитан
/// здесь один раз, отрисовка кадра только смешивает готовые значения.
struct GlyphAtlas {
    glyph_w: usize,
    glyph_h: usize,
    coverage: Vec<u8>,
}

impl GlyphAtlas {
    fn new(scale: usize) -> Self {
        let glyph_w = CELL_W * scale;
        let glyph_h = CELL_H * scale;
        let mut coverage = vec![0u8; GLYPH_COUNT * glyph_w * glyph_h];

        for (index, (_, rows)) in FONT.iter().enumerate() {
            let base = index * glyph_w * glyph_h;
            for (row, pattern) in rows.split('|').enumerate() {
                for (col, dot) in pattern.chars().enumerate() {
                    if dot != '#' {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let y = row * scale + dy;
                            let x = col * scale + dx;
                            coverage[base + y * glyph_w + x] = u8::MAX;
                        }
                    }
                }
            }
        }

        // На масштабе 1 знакоместо равно точке битмапа, размывать нечего:
        // ядро съело бы сам штрих, а не лесенку на его краю.
        if scale >= 2 {
            let kernel = scale | 1;
            for index in 0..GLYPH_COUNT {
                let base = index * glyph_w * glyph_h;
                let slot = &mut coverage[base..base + glyph_w * glyph_h];
                let blurred = box_blur(slot, glyph_w, glyph_h, kernel);
                slot.copy_from_slice(&blurred);
            }
        }

        Self {
            glyph_w,
            glyph_h,
            coverage,
        }
    }

    fn glyph(&self, ch: char) -> &[u8] {
        let code = u8::try_from(u32::from(ch)).unwrap_or(FALLBACK_GLYPH);
        let code = if (FIRST_GLYPH..=LAST_GLYPH).contains(&code) {
            code
        } else {
            FALLBACK_GLYPH
        };
        let size = self.glyph_w * self.glyph_h;
        let base = usize::from(code - FIRST_GLYPH) * size;
        &self.coverage[base..base + size]
    }
}

/// Разделимое box-размытие: край глифа получает полутон шириной в одну точку
/// исходного битмапа. Края маски продлеваются, чтобы штрих не темнел с торцов.
fn box_blur(src: &[u8], width: usize, height: usize, kernel: usize) -> Vec<u8> {
    let radius = kernel / 2;
    let mut horizontal = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0u32;
            for step in 0..kernel {
                let sx = (x + step).saturating_sub(radius).min(width - 1);
                sum += u32::from(src[y * width + sx]);
            }
            horizontal[y * width + x] = (sum / kernel as u32) as u8;
        }
    }

    let mut out = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0u32;
            for step in 0..kernel {
                let sy = (y + step).saturating_sub(radius).min(height - 1);
                sum += u32::from(horizontal[sy * width + x]);
            }
            out[y * width + x] = (sum / kernel as u32) as u8;
        }
    }
    out
}

/// Плашка с текстом поверх кадра YUV420p с плотной укладкой (`stride == width`).
pub struct TextOverlay {
    atlas: GlyphAtlas,
    width: usize,
    height: usize,
    scale: usize,
    plate_x: usize,
    plate_y: usize,
    plate_w: usize,
    plate_h: usize,
    /// Сколько знакомест влезает в строку; всё, что длиннее, обрезается.
    cols: usize,
    lines: Vec<String>,
    /// Покрытие глифами по площади плашки, переиспользуется между кадрами.
    coverage: Vec<u8>,
}

impl TextOverlay {
    /// Готовит плашку под размер кадра. `None`, если кадр слишком мал: на нём
    /// плашка съела бы картинку целиком.
    pub fn new(identity: &SourceIdentity, size: &FrameSize, rate: FrameRate) -> Option<Self> {
        let width = size.width as usize;
        let height = size.height as usize;
        let scale = (width / 640).min(height / 360).max(1);

        let outer_pad = OUTER_PAD_CELLS * scale;
        let inner_pad = INNER_PAD_CELLS * scale;

        let mut lines = vec![
            String::new(),
            format!("stream: {}", identity.stream()),
            format!("host:   {}", identity.host()),
            String::new(),
            format!(
                "{}x{} @ {:.2} fps",
                size.width,
                size.height,
                f64::from(rate.numerator) / f64::from(rate.denominator.max(1))
            ),
        ];

        // Ширина плашки считается по самой длинной строке, какой она может
        // стать, а не по текущей: иначе плашка дышала бы с каждым кадром.
        let widest = lines
            .iter()
            .map(|line| line.chars().count())
            .chain([TIME_LINE_WIDTH, "frame:  ".len() + FRAME_COUNTER_DIGITS])
            .max()
            .unwrap_or(0);

        let text_budget_w = width.checked_sub(2 * outer_pad + 2 * inner_pad)?;
        let cols = ((text_budget_w + scale) / (ADVANCE * scale)).min(widest);
        if cols == 0 {
            return None;
        }

        let text_budget_h = (height / MAX_PLATE_HEIGHT_DIVISOR)
            .checked_sub(outer_pad + 2 * inner_pad)?
            .checked_add(scale)?;
        let rows = text_budget_h / (LINE_PITCH * scale);
        if rows == 0 {
            return None;
        }
        lines.truncate(rows);

        let plate_w = cols * ADVANCE * scale - scale + 2 * inner_pad;
        let plate_h = lines.len() * LINE_PITCH * scale - scale + 2 * inner_pad;

        Some(Self {
            atlas: GlyphAtlas::new(scale),
            width,
            height,
            scale,
            plate_x: outer_pad,
            plate_y: outer_pad,
            plate_w,
            plate_h,
            cols,
            lines,
            coverage: vec![0u8; plate_w * plate_h],
        })
    }

    /// Рисует плашку в кадр. Время берётся из PTS кадра, а не из системных
    /// часов: тогда текст, нижняя полоса PTS и таймлайн плеера сходятся.
    pub fn draw(&mut self, frame: &mut [u8], pts_ms: u64, frame_index: u64) {
        if frame.len() < self.width * self.height * 3 / 2 {
            return;
        }
        self.refresh_dynamic_lines(pts_ms, frame_index);
        self.rasterize();
        self.blend_luma(frame);
        self.blend_chroma(frame);
    }

    fn refresh_dynamic_lines(&mut self, pts_ms: u64, frame_index: u64) {
        if let Some(line) = self.lines.get_mut(LINE_TIME) {
            line.clear();
            write_utc(line, pts_ms);
        }
        if let Some(line) = self.lines.get_mut(LINE_FRAME) {
            line.clear();
            let _ = write!(line, "frame:  {frame_index}");
        }
    }

    fn rasterize(&mut self) {
        self.coverage.fill(0);
        let inner_pad = INNER_PAD_CELLS * self.scale;
        let (glyph_w, glyph_h) = (self.atlas.glyph_w, self.atlas.glyph_h);

        for (row, line) in self.lines.iter().enumerate() {
            let origin_y = inner_pad + row * LINE_PITCH * self.scale;
            for (col, ch) in line.chars().take(self.cols).enumerate() {
                let origin_x = inner_pad + col * ADVANCE * self.scale;
                let mask = self.atlas.glyph(ch);
                for gy in 0..glyph_h {
                    let y = origin_y + gy;
                    if y >= self.plate_h {
                        break;
                    }
                    for gx in 0..glyph_w {
                        let x = origin_x + gx;
                        if x >= self.plate_w {
                            break;
                        }
                        let value = mask[gy * glyph_w + gx];
                        let slot = &mut self.coverage[y * self.plate_w + x];
                        *slot = (*slot).max(value);
                    }
                }
            }
        }
    }

    fn blend_luma(&self, frame: &mut [u8]) {
        for y in 0..self.plate_h {
            let row = (self.plate_y + y) * self.width + self.plate_x;
            for x in 0..self.plate_w {
                let pixel = &mut frame[row + x];
                // Подложка непрозрачна, поэтому исходная картинка под плашкой
                // роли не играет и результат зависит только от текста.
                *pixel = blend(PLATE_LUMA, GLYPH_LUMA, self.coverage[y * self.plate_w + x]);
            }
        }
    }

    fn blend_chroma(&self, frame: &mut [u8]) {
        let luma_size = self.width * self.height;
        let chroma_w = self.width / 2;
        let chroma_h = self.height / 2;
        let u_base = luma_size;
        let v_base = luma_size + chroma_w * chroma_h;

        let cy0 = self.plate_y / 2;
        let cy1 = ((self.plate_y + self.plate_h) / 2).min(chroma_h);
        let cx0 = self.plate_x / 2;
        let cx1 = ((self.plate_x + self.plate_w) / 2).min(chroma_w);

        for cy in cy0..cy1 {
            for cx in cx0..cx1 {
                frame[u_base + cy * chroma_w + cx] = NEUTRAL_CHROMA;
                frame[v_base + cy * chroma_w + cx] = NEUTRAL_CHROMA;
            }
        }
    }
}

fn blend(background: u8, foreground: u8, alpha: u8) -> u8 {
    let alpha = u32::from(alpha);
    let mixed = (u8::MAX as u32 - alpha) * u32::from(background) + alpha * u32::from(foreground);
    ((mixed + 127) / u8::MAX as u32) as u8
}

fn write_utc(out: &mut String, pts_ms: u64) {
    let seconds = pts_ms / 1000;
    let days = seconds / 86400;
    // Gregorian civil date, with Unix day zero at 1970-01-01.
    let z = days as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let _ = write!(
        out,
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03} UTC",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        pts_ms % 1000
    );
}
