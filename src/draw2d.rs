//! web3d-M6: the 2D draw-command list.
//!
//! Every 2D draw builtin (`rect`, `circle`, `line`, `text`, sprites,
//! atlases, tilemaps, nine-slice panels, the UI widgets) and the `fx` /
//! `light2d` overlays push [`Draw2d`] commands into `Env::draw2d`
//! instead of drawing. The frame's list is then drawn by the kernel's
//! 2D canvas (`kernel/canvas.rs`): as a 2D game's whole frame (native
//! `play2d`, the browser), and as the HUD over a 3D game.
//!
//! Coordinates are the script's: the 640×480 virtual canvas, or world
//! space under an active `camera2d` (the renderer applies the camera).
//! Text `y` is the baseline. Commands hold plain data (asset paths,
//! linear-in-script sRGB `[r, g, b, a]` colours), never renderer
//! handles, so any renderer can draw them.

/// A colour as the script wrote it (sRGB, 0–1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub [f32; 4]);

impl From<crate::color::Color> for Rgba {
    fn from(c: crate::color::Color) -> Self {
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

/// The built-in font (ProggyClean, MIT; macroquad's default too), parsed
/// once. Text drawn without a font and [`measure_text`] use it.
pub fn default_font() -> &'static fontdue::Font {
    static FONT: std::sync::OnceLock<fontdue::Font> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        fontdue::Font::from_bytes(
            &include_bytes!("kernel/ProggyClean.ttf")[..],
            fontdue::FontSettings::default(),
        )
        .expect("the bundled font parses")
    })
}

/// A run of text's extent in the built-in font at `size` pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextSize {
    /// Sum of the glyph advances.
    pub width: f32,
    /// From the lowest glyph bottom to the highest glyph top.
    pub height: f32,
    /// The highest glyph top above the baseline.
    pub offset_y: f32,
}

/// Measure `text` as macroquad's `measure_text` did, so the UI widgets
/// centre their labels on the same pixels. An empty string measures
/// zero wide (and a meaningless height, as in macroquad).
pub fn measure_text(text: &str, size: u16) -> TextSize {
    let font = default_font();
    let px = f32::from(size);
    let (mut width, mut min_y, mut max_y) = (0.0, f32::MAX, f32::MIN);
    for ch in text.chars() {
        let m = font.metrics(ch, px);
        width += m.advance_width;
        min_y = f32::min(min_y, m.ymin as f32);
        max_y = f32::max(max_y, m.height as f32 + m.ymin as f32);
    }
    TextSize {
        width,
        height: max_y - min_y,
        offset_y: max_y,
    }
}
