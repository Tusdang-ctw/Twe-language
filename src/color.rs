//! web3d-M6: a plain RGBA colour, replacing macroquad's `Color` once
//! macroquad was removed. Channels are the script's colours (sRGB,
//! 0..1); the kernel canvas decodes them when it draws (`kernel/canvas`).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a }
    }
}

pub const YELLOW: Color = Color::new(1.0, 1.0, 0.0, 1.0);
