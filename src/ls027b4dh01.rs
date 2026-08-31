use crate::font::{MaskOp, TextTarget, blend_mask_into_row};

pub const SHARP_COLS: usize = 400;
pub const SHARP_LINE_BYTES: usize = SHARP_COLS / 8;
pub const SHARP_ROWS: usize = 240;

#[derive(Copy, Clone)]
#[repr(C)]
pub struct SharpDisplayLine {
    modes: u8,
    line_addr: u8,
    data: [u8; SHARP_LINE_BYTES],
    padding: u16,
}

pub struct SharpDisplayDriver {
    lines: [SharpDisplayLine; SHARP_ROWS],
    dirty: [bool; SHARP_ROWS],
    current_vcom: bool,
    dirty_line_iter: Option<usize>,
}

impl TextTarget for SharpDisplayDriver {
    fn blend_mask_row(&mut self, x: i32, y: i32, mask: &[u8], width: usize, op: MaskOp) {
        let Ok(row) = usize::try_from(y) else {
            return;
        };
        if row >= SHARP_ROWS {
            return;
        }

        if blend_mask_into_row(&mut self.lines[row].data, x, mask, width, op) {
            self.dirty[row] = true;
        }
    }
}

impl SharpDisplayDriver {
    pub fn new() -> SharpDisplayDriver {
        let mut driver = SharpDisplayDriver {
            lines: [SharpDisplayLine {
                modes: 0,
                line_addr: 0,
                data: [0xff; SHARP_LINE_BYTES],
                padding: 0,
            }; SHARP_ROWS],
            dirty: [false; SHARP_ROWS],
            current_vcom: false,
            dirty_line_iter: None,
        };
        for i in 0..SHARP_ROWS {
            driver.lines[i].line_addr = (i + 1) as u8;
        }
        driver
    }

    pub fn set_pixel(&mut self, px: usize, py: usize, black: bool) {
        let byte_index = px / 8;
        let bit_offs = (px % 8) as u8;
        let bit = 1 << bit_offs;
        let line = &mut self.lines[py];
        let byte = &mut line.data[byte_index];
        if !black && bit & *byte == 0 {
            *byte |= bit;
            self.dirty[py] = true;
        } else if black && bit & *byte != 0 {
            *byte &= !bit;
            self.dirty[py] = true;
        }
    }

    pub fn set_byte(&mut self, bx: usize, py: usize, b: u8) {}

    pub fn set_fullscreen(&mut self, b: &[u8]) {
        assert_eq!(b.len(), SHARP_ROWS * SHARP_LINE_BYTES);
        for y in 0..SHARP_ROWS {
            let start = y * SHARP_LINE_BYTES;
            let src = &b[start..start + SHARP_LINE_BYTES];
            self.lines[y].data.copy_from_slice(src);
        }
        self.dirty = [true; SHARP_ROWS];
        self.dirty_line_iter = None;
    }

    fn set_vcom_bit(current_vcom: bool, b: &mut u8) {
        if current_vcom {
            *b |= 2;
        } else {
            *b &= !2;
        }
    }

    pub fn next_dirty_bytes(&mut self) -> Option<&[u8]> {
        let start_idx: usize = match self.dirty_line_iter {
            None => 0,
            Some(x) => x + 1,
        };
        let idx = match (start_idx..SHARP_ROWS).find(|&i| self.dirty[i]) {
            None => {
                self.dirty_line_iter = None;
                return None;
            }
            Some(i) => i,
        };

        self.dirty_line_iter = Some(idx);
        self.dirty[idx] = false;

        let vcom = self.current_vcom;
        let line = &mut self.lines[idx];
        line.modes = 1;
        Self::set_vcom_bit(vcom, &mut line.modes);

        let line = &self.lines[idx];
        Some(unsafe {
            core::slice::from_raw_parts(
                line as *const SharpDisplayLine as *const u8,
                core::mem::size_of::<SharpDisplayLine>(),
            )
        })
    }

    pub fn vcom_cmd(&self) -> [u8; 2] {
        let mut ret = [0; 2];
        Self::set_vcom_bit(self.current_vcom, &mut ret[0]);
        ret
    }

    pub fn swap_vcom(&mut self) {
        self.current_vcom = !self.current_vcom;
    }

    pub fn all_clear_cmd(&mut self) -> [u8; 2] {
        for line in self.lines.iter_mut() {
            line.data = [0xff; SHARP_LINE_BYTES];
        }
        self.dirty = [false; SHARP_ROWS];
        self.dirty_line_iter = None;

        let mut ret = [0; 2];
        Self::set_vcom_bit(self.current_vcom, &mut ret[0]);
        ret[0] |= 0x4;
        ret
    }
}
