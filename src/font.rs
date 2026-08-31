pub enum FontAdvances {
    Fixed(u8),
    Variable(&'static [u8]),
}

pub struct Font {
    pub width: u8, // cell width
    pub rows: u8, // rows per glyph
    pub top: u8, // offset from top of line to first stored row
    pub line_height: u8, // untrimmed line height
    pub baseline: u8, // baseline (bottom of text) from top of line
    pub row_repeat: u8, // # of rows per stored bitmap row
    pub tracking: i8, // extra pixels added after glyph advance
    pub first: u32, // first unicode codepoint 
    pub count: u16,
    pub advances: FontAdvances,
    /// Glyph-major, row-major, LSB-left, one-bit ink masks.
    pub data: &'static [u8],
}

impl Font {
    pub const fn row_bytes(&self) -> usize {
        (self.width as usize + 7) / 8
    }

    pub const fn glyph_bytes(&self) -> usize {
        self.row_bytes() * self.rows as usize
    }

    pub fn glyph_index(&self, ch: char) -> Option<usize> {
        let index = (ch as u32).checked_sub(self.first)?;
        (index < self.count as u32).then_some(index as usize)
    }

    pub fn glyph_data(&self, index: usize) -> Option<&[u8]> {
        if index >= self.count as usize {
            return None;
        }

        let start = index.checked_mul(self.glyph_bytes())?;
        self.data.get(start..start + self.glyph_bytes())
    }

    pub fn glyph_advance(&self, index: usize) -> Option<u8> {
        if index >= self.count as usize {
            return None;
        }

        match &self.advances {
            FontAdvances::Fixed(advance) => Some(*advance),
            FontAdvances::Variable(advances) => advances.get(index).copied(),
        }
    }

    fn resolve_glyph(&self, ch: char) -> Option<usize> {
        self.glyph_index(ch)
            .or_else(|| self.glyph_index('?'))
            .or_else(|| self.glyph_index(' '))
    }
}

#[derive(Clone, Copy)]
pub enum MaskOp {
    PaintBlack,
    PaintWhite,
}

pub trait TextTarget {
    /// Blend `width` LSB-first mask bits into one display row. Set mask bits
    /// are affected and clear mask bits are transparent. The target clips.
    fn blend_mask_row(&mut self, x: i32, y: i32, mask: &[u8], width: usize, op: MaskOp);
}

#[derive(Clone, Copy)]
pub struct TextOptions {
    /// True paints set mask bits black; false paints them white.
    pub black: bool,
    /// Integer scale. Zero is treated as one.
    pub scale: u8,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            black: true,
            scale: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextMeasurement {
    /// Distance from the top of the first line box to its baseline.
    pub baseline_offset: u16,
    /// Largest logical line advance, including inter-glyph tracking.
    pub width: u16,
    /// Total height of all line boxes. Empty text has zero height.
    pub height: u16,
}

fn mask_window(mask: &[u8], source_bit: usize) -> u8 {
    let byte = source_bit / 8;
    let shift = source_bit & 7;
    let low = mask.get(byte).copied().unwrap_or(0) as u16;
    let high = mask.get(byte + 1).copied().unwrap_or(0) as u16;
    ((low | (high << 8)) >> shift) as u8
}

/// Blend a mask into an LSB-left, 1-bit row where set destination bits are
/// white. Returns true when at least one destination byte changed.
pub(crate) fn blend_mask_into_row(
    destination: &mut [u8],
    x: i32,
    mask: &[u8],
    width: usize,
    op: MaskOp,
) -> bool {
    let width = width.min(mask.len().saturating_mul(8));
    if width == 0 || destination.is_empty() {
        return false;
    }

    let mask_left = i64::from(x);
    let mask_right = mask_left + width as i64;
    let clipped_left = mask_left.max(0);
    let clipped_right = mask_right.min(destination.len() as i64 * 8);
    if clipped_left >= clipped_right {
        return false;
    }

    let clipped_left = clipped_left as usize;
    let clipped_right = clipped_right as usize;
    let source_start = (clipped_left as i64 - mask_left) as usize;
    let first_byte = clipped_left / 8;
    let last_byte = (clipped_right - 1) / 8;
    let mut changed = false;

    for destination_byte in first_byte..=last_byte {
        let destination_left = destination_byte * 8;
        let overlap_left = clipped_left.max(destination_left);
        let overlap_right = clipped_right.min(destination_left + 8);
        let bit_count = overlap_right - overlap_left;
        let destination_shift = overlap_left - destination_left;
        let source_bit = source_start + overlap_left - clipped_left;
        let valid_bits = ((((1u16 << bit_count) - 1) << destination_shift) & 0xff) as u8;
        let ink = (mask_window(mask, source_bit) << destination_shift) & valid_bits;
        let old = destination[destination_byte];
        let new = match op {
            MaskOp::PaintBlack => old & !ink,
            MaskOp::PaintWhite => old | ink,
        };

        if new != old {
            destination[destination_byte] = new;
            changed = true;
        }
    }

    changed
}

fn saturating_u16(value: i32) -> u16 {
    value.clamp(0, u16::MAX as i32) as u16
}

fn layout_character<F>(
    font: &Font,
    ch: char,
    scale: i32,
    origin_x: i32,
    line_y: i32,
    cursor_x: &mut i32,
    first_on_line: &mut bool,
    maximum_width: &mut i32,
    visit: &mut F,
) where
    F: FnMut(usize, i32, i32),
{
    let Some(index) = font.resolve_glyph(ch) else {
        return;
    };

    if !*first_on_line {
        *cursor_x = cursor_x.saturating_add(i32::from(font.tracking) * scale);
    }

    visit(index, origin_x.saturating_add(*cursor_x), line_y);
    let advance = i32::from(font.glyph_advance(index).unwrap_or(0)) * scale;
    *cursor_x = cursor_x.saturating_add(advance);
    *maximum_width = (*maximum_width).max(*cursor_x);
    *first_on_line = false;
}

fn layout_text<F>(
    origin_x: i32,
    origin_y: i32,
    font: &Font,
    text: &str,
    options: TextOptions,
    mut visit: F,
) -> TextMeasurement
where
    F: FnMut(usize, i32, i32),
{
    let scale = i32::from(options.scale.max(1));
    let line_advance = i32::from(font.line_height) * scale;
    let mut cursor_x = 0i32;
    let mut line_y = origin_y;
    let mut first_on_line = true;
    let mut maximum_width = 0i32;
    let mut line_count = 0i32;

    for ch in text.chars() {
        if ch == '\r' {
            continue;
        }

        if line_count == 0 {
            line_count = 1;
        }

        if ch == '\n' {
            maximum_width = maximum_width.max(cursor_x);
            cursor_x = 0;
            line_y = line_y.saturating_add(line_advance);
            first_on_line = true;
            line_count = line_count.saturating_add(1);
            continue;
        }

        if ch == '\t' {
            for _ in 0..4 {
                layout_character(
                    font,
                    ' ',
                    scale,
                    origin_x,
                    line_y,
                    &mut cursor_x,
                    &mut first_on_line,
                    &mut maximum_width,
                    &mut visit,
                );
            }
            continue;
        }

        layout_character(
            font,
            ch,
            scale,
            origin_x,
            line_y,
            &mut cursor_x,
            &mut first_on_line,
            &mut maximum_width,
            &mut visit,
        );
    }

    maximum_width = maximum_width.max(cursor_x);
    TextMeasurement {
        baseline_offset: if line_count == 0 {
            0
        } else {
            u16::from(font.baseline) * options.scale.max(1) as u16
        },
        width: saturating_u16(maximum_width),
        height: saturating_u16(line_count.saturating_mul(line_advance)),
    }
}

/// Measure text using the same character, fallback, tracking, tab, newline,
/// and scaling rules as [`draw_text`].
pub fn measure_text(font: &Font, text: &str, options: TextOptions) -> TextMeasurement {
    layout_text(0, 0, font, text, options, |_, _, _| {})
}

fn draw_glyph<T: TextTarget>(
    target: &mut T,
    x: i32,
    line_y: i32,
    font: &Font,
    glyph_index: usize,
    options: TextOptions,
) {
    let Some(glyph) = font.glyph_data(glyph_index) else {
        return;
    };

    let scale = options.scale.max(1) as usize;
    let row_bytes = font.row_bytes();
    let operation = if options.black {
        MaskOp::PaintBlack
    } else {
        MaskOp::PaintWhite
    };
    let first_y = line_y.saturating_add(i32::from(font.top) * scale as i32);
    let vertical_repeat = font.row_repeat as usize * scale;

    for source_y in 0..font.rows as usize {
        let row_start = source_y * row_bytes;
        let row = &glyph[row_start..row_start + row_bytes];
        let destination_y =
            first_y.saturating_add((source_y * vertical_repeat).min(i32::MAX as usize) as i32);

        for repeat in 0..vertical_repeat {
            let y = destination_y.saturating_add(repeat.min(i32::MAX as usize) as i32);
            if scale == 1 {
                target.blend_mask_row(x, y, row, font.width as usize, operation);
            } else {
                draw_scaled_mask_row(target, x, y, row, font.width as usize, scale, operation);
            }
        }
    }
}

fn draw_scaled_mask_row<T: TextTarget>(
    target: &mut T,
    x: i32,
    y: i32,
    mask: &[u8],
    width: usize,
    scale: usize,
    operation: MaskOp,
) {
    // One source byte expands to at most 255 bytes because scale is a u8.
    let mut expanded = [0u8; u8::MAX as usize];
    let mut source_x = 0usize;

    while source_x < width {
        let chunk_width = (width - source_x).min(8);
        let expanded_width = chunk_width * scale;
        let expanded_bytes = expanded_width.div_ceil(8);
        expanded[..expanded_bytes].fill(0);

        for bit in 0..chunk_width {
            let source_bit = source_x + bit;
            if mask[source_bit / 8] & (1 << (source_bit & 7)) == 0 {
                continue;
            }

            for repeated_bit in bit * scale..(bit + 1) * scale {
                expanded[repeated_bit / 8] |= 1 << (repeated_bit & 7);
            }
        }

        let destination_x = x.saturating_add((source_x * scale).min(i32::MAX as usize) as i32);
        target.blend_mask_row(
            destination_x,
            y,
            &expanded[..expanded_bytes],
            expanded_width,
            operation,
        );
        source_x += chunk_width;
    }
}

/// Transparently draw text with `(x, y)` at the top-left of the first line
/// box. Only set glyph-mask bits modify the target.
pub fn draw_text<T: TextTarget>(
    target: &mut T,
    x: i32,
    y: i32,
    font: &Font,
    text: &str,
    options: TextOptions,
) {
    let _ = layout_text(x, y, font, text, options, |index, glyph_x, line_y| {
        draw_glyph(target, glyph_x, line_y, font, index, options);
    });
}

