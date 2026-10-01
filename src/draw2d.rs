//! web3d-M6: the 2D draw-command list.
//!
//! Every 2D draw builtin (`rect`, `circle`, `line`, `text`, sprites,
//! atlases, tilemaps, nine-slice panels, the UI widgets) and the `fx` /
//! `light2d` overlays push [`Draw2d`] commands into `Env::draw2d`
//! instead of drawing. The frame's list is then drawn by whichever
//! renderer runs the game:
//!
//! - in a 3D game, the kernel draws it over the scene (`kernel/hud.rs`);
//! - in a 2D game, the macroquad play loop replays it
//!   ([`replay_macroquad`]), until M6 moves 2D onto the kernel too.
//!
//! Coordinates are the script's: the 640×480 virtual canvas, or world
//! space under an active `camera2d` (the renderer applies the camera).
//! Text `y` is the baseline. Commands hold plain data (asset paths,
//! linear-in-script sRGB `[r, g, b, a]` colours), never renderer
//! handles, so any renderer can draw them.

/// A colour as the script wrote it (sRGB, 0–1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub [f32; 4]);

impl From<macroquad::color::Color> for Rgba {
    fn from(c: macroquad::color::Color) -> Self {
        Rgba([c.r, c.g, c.b, c.a])
    }
}

impl From<[f32; 4]> for Rgba {
    fn from(c: [f32; 4]) -> Self {
        Rgba(c)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Draw2d {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    RectLines {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        thickness: f32,
        color: [f32; 4],
    },
    Circle {
        x: f32,
        y: f32,
        r: f32,
        color: [f32; 4],
    },
    CircleLines {
        x: f32,
        y: f32,
        r: f32,
        thickness: f32,
        color: [f32; 4],
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        thickness: f32,
        color: [f32; 4],
    },
    /// `(x, y)` is the start of the baseline. `font` is a font's asset
    /// path (`load_font`), or `None` for the built-in font.
    Text {
        text: String,
        x: f32,
        y: f32,
        size: f32,
        color: [f32; 4],
        font: Option<String>,
    },
    /// An image (by asset path) drawn with its top-left at `(x, y)`:
    /// at `dest` size if given (else the source's size), from the
    /// `source` rectangle `[x, y, w, h]` in pixels if given (else the
    /// whole image), multiplied by `tint`.
    Texture {
        path: String,
        x: f32,
        y: f32,
        dest: Option<[f32; 2]>,
        source: Option<[f32; 4]>,
        tint: [f32; 4],
    },
}

pub fn rect(out: &mut Vec<Draw2d>, x: f32, y: f32, w: f32, h: f32, color: impl Into<Rgba>) {
    out.push(Draw2d::Rect {
        x,
        y,
        w,
        h,
        color: color.into().0,
    });
}

pub fn rect_lines(
    out: &mut Vec<Draw2d>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    thickness: f32,
    color: impl Into<Rgba>,
) {
    out.push(Draw2d::RectLines {
        x,
        y,
        w,
        h,
        thickness,
        color: color.into().0,
    });
}

pub fn circle(out: &mut Vec<Draw2d>, x: f32, y: f32, r: f32, color: impl Into<Rgba>) {
    out.push(Draw2d::Circle {
        x,
        y,
        r,
        color: color.into().0,
    });
}

pub fn circle_lines(
    out: &mut Vec<Draw2d>,
    x: f32,
    y: f32,
    r: f32,
    thickness: f32,
    color: impl Into<Rgba>,
) {
    out.push(Draw2d::CircleLines {
        x,
        y,
        r,
        thickness,
        color: color.into().0,
    });
}

#[allow(clippy::too_many_arguments)]
pub fn line(
    out: &mut Vec<Draw2d>,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    thickness: f32,
    color: impl Into<Rgba>,
) {
    out.push(Draw2d::Line {
        x1,
        y1,
        x2,
        y2,
        thickness,
        color: color.into().0,
    });
}

pub fn text(out: &mut Vec<Draw2d>, text: &str, x: f32, y: f32, size: f32, color: impl Into<Rgba>) {
    out.push(Draw2d::Text {
        text: text.to_string(),
        x,
        y,
        size,
        color: color.into().0,
        font: None,
    });
}

#[allow(clippy::too_many_arguments)]
pub fn text_font(
    out: &mut Vec<Draw2d>,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    color: impl Into<Rgba>,
    font: &str,
) {
    out.push(Draw2d::Text {
        text: text.to_string(),
        x,
        y,
        size,
        color: color.into().0,
        font: Some(font.to_string()),
    });
}

pub fn texture(
    out: &mut Vec<Draw2d>,
    path: &str,
    x: f32,
    y: f32,
    dest: Option<[f32; 2]>,
    source: Option<[f32; 4]>,
) {
    out.push(Draw2d::Texture {
        path: path.to_string(),
        x,
        y,
        dest,
        source,
        tint: [1.0, 1.0, 1.0, 1.0],
    });
}

/// Draw `cmds` with macroquad, in order (the 2D play loop calls this
/// under the frame's camera). Textures and fonts come from the stdlib's
/// caches, which their builtins filled when the command was pushed.
pub fn replay_macroquad(cmds: &[Draw2d]) {
    use macroquad::color::Color;
    let c = |v: &[f32; 4]| Color::new(v[0], v[1], v[2], v[3]);
    for cmd in cmds {
        match cmd {
            Draw2d::Rect { x, y, w, h, color } => {
                macroquad::shapes::draw_rectangle(*x, *y, *w, *h, c(color))
            }
            Draw2d::RectLines {
                x,
                y,
                w,
                h,
                thickness,
                color,
            } => macroquad::shapes::draw_rectangle_lines(*x, *y, *w, *h, *thickness, c(color)),
            Draw2d::Circle { x, y, r, color } => {
                macroquad::shapes::draw_circle(*x, *y, *r, c(color))
            }
            Draw2d::CircleLines {
                x,
                y,
                r,
                thickness,
                color,
            } => macroquad::shapes::draw_circle_lines(*x, *y, *r, *thickness, c(color)),
            Draw2d::Line {
                x1,
                y1,
                x2,
                y2,
                thickness,
                color,
            } => macroquad::shapes::draw_line(*x1, *y1, *x2, *y2, *thickness, c(color)),
            Draw2d::Text {
                text,
                x,
                y,
                size,
                color,
                font: None,
            } => {
                macroquad::text::draw_text(text, *x, *y, *size, c(color));
            }
            Draw2d::Text {
                text,
                x,
                y,
                size,
                color,
                font: Some(path),
            } => {
                crate::stdlib::with_decoded_font(path, |font| {
                    macroquad::text::draw_text_ex(
                        text,
                        *x,
                        *y,
                        macroquad::text::TextParams {
                            font: Some(font),
                            font_size: *size as u16,
                            color: c(color),
                            ..Default::default()
                        },
                    );
                });
            }
            Draw2d::Texture {
                path,
                x,
                y,
                dest,
                source,
                tint,
            } => {
                crate::stdlib::with_cached_texture(path, |tex| {
                    macroquad::texture::draw_texture_ex(
                        tex,
                        *x,
                        *y,
                        c(tint),
                        macroquad::texture::DrawTextureParams {
                            dest_size: dest.map(|d| macroquad::math::vec2(d[0], d[1])),
                            source: source.map(|s| macroquad::math::Rect {
                                x: s[0],
                                y: s[1],
                                w: s[2],
                                h: s[3],
                            }),
                            ..Default::default()
                        },
                    );
                });
            }
        }
    }
}
