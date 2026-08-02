
pub struct Font {
    pub width: u8,
    pub rows: u8,
    pub y_offset: u8,
    pub advance: u8,
    pub first: u8,
    pub count: u8,
    pub data: &'static [u8],
}


