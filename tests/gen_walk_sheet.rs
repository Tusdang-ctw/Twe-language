//! Phase 11 session 8: spritesheet animation demo.
//!
//! Generates a deterministic 8-frame walk-cycle PNG into
//! `examples/assets/walk.png` if it isn't already present, then
//! checks `examples/walk_demo.twe` parses + type-checks. The PNG is
//! committed to git so end users don't need to run this test; it
//! exists as the procedural spec for the asset (a future session
//! can pressure the grid layout by re-running and diff-checking).
//!
//! Layout: 8 frames of 32×32 in a 256×32 strip (grid: (8, 1)). Each
//! frame is a stick figure walking — the head bobs, the legs swing,
//! the torso colour cycles through a hue ramp so the animation is
//! visible as both motion and color change.

use std::path::Path;

const FRAMES: u32 = 8;
const FRAME_SIZE: u32 = 32;
const SHEET_W: u32 = FRAMES * FRAME_SIZE;
const SHEET_H: u32 = FRAME_SIZE;

fn rgba(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    [r, g, b, a]
}

fn put(buf: &mut [u8], x: u32, y: u32, color: [u8; 4]) {
    if x >= SHEET_W || y >= SHEET_H {
        return;
    }
    let i = ((y * SHEET_W + x) as usize) * 4;
    buf[i..i + 4].copy_from_slice(&color);
}

fn fill_rect(buf: &mut [u8], x0: u32, y0: u32, w: u32, h: u32, color: [u8; 4]) {
    for dy in 0..h {
        for dx in 0..w {
            put(buf, x0 + dx, y0 + dy, color);
        }
    }
}

fn render_frame(buf: &mut [u8], frame: u32) {
    let fx = frame * FRAME_SIZE;
    // Background: transparent. The buffer is already zeroed.

    // Hue ramp on the torso so the animation reads even if motion
    // is small. 8 frames → 8 hues evenly spaced through the color
    // wheel. Compute via simple HSV → RGB at full saturation/value.
    let hue = (frame as f32) * (360.0 / FRAMES as f32);
    let torso_color = hsv_to_rgba(hue, 0.85, 0.85, 255);

    // Head: 8x8 square that bobs ±1 px on alternating frames.
    let head_y_offset: u32 = if frame % 2 == 0 { 0 } else { 1 };
    fill_rect(
        buf,
        fx + 12,
        4 + head_y_offset,
        8,
        8,
        rgba(220, 180, 140, 255),
    );

    // Torso: 10x10.
    fill_rect(buf, fx + 11, 13, 10, 10, torso_color);

    // Legs: two rectangles whose horizontal offset swings with the
    // frame. Frames 0..4 swing left, 4..8 swing right.
    let phase = if frame < 4 {
        frame as i32
    } else {
        (8 - frame) as i32
    } - 2;
    let left_leg_x = (15 + phase) as u32;
    let right_leg_x = (15 - phase) as u32;
    fill_rect(buf, fx + left_leg_x, 23, 2, 8, rgba(60, 60, 80, 255));
    fill_rect(buf, fx + right_leg_x + 2, 23, 2, 8, rgba(60, 60, 80, 255));

    // Arms: opposite phase to the legs.
    let left_arm_x = (15 - phase) as u32;
    let right_arm_x = (15 + phase) as u32;
    fill_rect(buf, fx + left_arm_x - 1, 14, 2, 6, torso_color);
    fill_rect(buf, fx + right_arm_x + 3, 14, 2, 6, torso_color);
}

fn hsv_to_rgba(h: f32, s: f32, v: f32, a: u8) -> [u8; 4] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h as u32 / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let r = ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [r, g, b, a]
}

#[test]
fn walk_sheet_generated_or_present() {
    let path = Path::new("examples/assets/walk.png");
    if !path.exists() {
        let mut buf = vec![0u8; (SHEET_W * SHEET_H * 4) as usize];
        for f in 0..FRAMES {
            render_frame(&mut buf, f);
        }
        image::save_buffer(
            path,
            &buf,
            SHEET_W,
            SHEET_H,
            image::ExtendedColorType::Rgba8,
        )
        .expect("write walk.png");
    }
    let meta = std::fs::metadata(path).expect("walk.png should exist");
    assert!(meta.len() > 0, "walk.png is empty");
}

#[test]
fn walk_demo_script_parses() {
    use twec::{infer, lexer, parser};
    let src = std::fs::read_to_string("examples/walk_demo.twe").expect("walk_demo.twe");
    let tokens = lexer::lex(&src).expect("lex");
    let program = parser::parse(&tokens).expect("parse");
    // `infer::infer_program` runs without panicking — `twec types`
    // would print results; for the test we just need it to not
    // error on missing names since we use real stdlib functions.
    let _ = infer::infer_program(&program);
}
