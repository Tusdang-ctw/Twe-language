//! web3d-M6: the kernel's 2D canvas (`kernel::canvas`).
//!
//! Two kinds of test:
//! - exact pixels for each `Draw2d` command, drawn the way macroquad
//!   drew it (half-thickness rectangle outlines, 20-sided circles, …);
//! - the reference comparison: every 2D example with a reference frame
//!   in `tests/references/m6/` runs headless through `frame2d` (the same
//!   frame step `twec play` uses, no input, one fixed tick per frame),
//!   the kernel draws the frame, and the picture is scored against the
//!   macroquad capture. Kernel frames and a report go to
//!   `target/m6-compare/`.
//!
//! Needs a GPU adapter; without one the tests print a notice and pass.

use twec::draw2d::Draw2d;
use twec::kernel::canvas::{Camera2d, Offscreen};

const W: u32 = 640;
const H: u32 = 480;

fn offscreen() -> Option<Offscreen> {
    match pollster::block_on(Offscreen::new(W, H)) {
        Ok(o) => Some(o),
        Err(e) => {
            eprintln!("skipping canvas test: {e}");
            None
        }
    }
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

#[test]
fn shapes_land_where_macroquad_put_them() {
    let Some(mut canvas) = offscreen() else {
        return;
    };
    let frame = canvas
        .render(
            &[
                Draw2d::Rect {
                    x: 10.0,
                    y: 10.0,
                    w: 20.0,
                    h: 10.0,
                    color: RED,
                },
                // macroquad's outline is half the thickness, inside.
                Draw2d::RectLines {
                    x: 100.0,
                    y: 10.0,
                    w: 40.0,
                    h: 40.0,
                    thickness: 8.0,
                    color: WHITE,
                },
                Draw2d::Circle {
                    x: 300.0,
                    y: 100.0,
                    r: 30.0,
                    color: WHITE,
                },
                Draw2d::CircleLines {
                    x: 400.0,
                    y: 100.0,
                    r: 30.0,
                    thickness: 4.0,
                    color: WHITE,
                },
                Draw2d::Line {
                    x1: 10.0,
                    y1: 200.0,
                    x2: 200.0,
                    y2: 200.0,
                    thickness: 4.0,
                    color: WHITE,
                },
            ],
            None,
        )
        .expect("render");
    // The filled rectangle covers exactly [10, 30) × [10, 20).
    assert_eq!(px(&frame, 10, 10), [255, 0, 0, 255]);
    assert_eq!(px(&frame, 29, 19), [255, 0, 0, 255]);
    assert_eq!(px(&frame, 30, 10)[0], 0);
    assert_eq!(px(&frame, 10, 20)[0], 0);
    // Outline: 4 px wide (half of 8), hollow inside.
    assert_eq!(px(&frame, 101, 30), [255, 255, 255, 255]);
    assert_eq!(px(&frame, 103, 30), [255, 255, 255, 255]);
    assert_eq!(px(&frame, 105, 30)[0], 0);
    assert_eq!(px(&frame, 120, 30)[0], 0);
    // Filled circle centre; the outline ring sits outside its radius.
    assert_eq!(px(&frame, 300, 100)[0], 255);
    assert_eq!(px(&frame, 400, 100)[0], 0);
    assert_eq!(px(&frame, 432, 100)[0], 255, "ring from r to r + thickness");
    assert_eq!(px(&frame, 427, 100)[0], 0);
    // A 4 px line centred on y = 200.
    assert_eq!(px(&frame, 100, 198)[0], 255);
    assert_eq!(px(&frame, 100, 201)[0], 255);
    assert_eq!(px(&frame, 100, 203)[0], 0);
}

#[test]
fn alpha_blends_in_gamma_space_like_macroquad() {
    let Some(mut canvas) = offscreen() else {
        return;
    };
    let frame = canvas
        .render(
            &[
                Draw2d::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 64.0,
                    h: 64.0,
                    color: WHITE,
                },
                Draw2d::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 64.0,
                    h: 64.0,
                    color: [0.0, 0.0, 0.0, 0.5],
                },
            ],
            None,
        )
        .expect("render");
    let [r, ..] = px(&frame, 32, 32);
    assert!(
        (127..=128).contains(&r),
        "half black over white is 50% grey in gamma space, got {r}"
    );
}

#[test]
fn text_has_its_baseline_at_y() {
    let Some(mut canvas) = offscreen() else {
        return;
    };
    let frame = canvas
        .render(
            &[Draw2d::Text {
                text: "HHHH".to_string(),
                x: 20.0,
                y: 100.0,
                size: 32.0,
                color: WHITE,
                font: None,
            }],
            None,
        )
        .expect("render");
    let lit = |y: u32| (20..120).any(|x| px(&frame, x, y)[0] > 128);
    assert!(lit(95), "glyphs sit above the baseline");
    assert!(!lit(102), "nothing below the baseline for H");
    assert!(!lit(70), "a 32 px H is shorter than 30 px");
}

/// A 4×4 test image: left half red, right half blue, written once.
fn quadrant_png() -> String {
    let path = "target/m6-compare/quadrants.png";
    std::fs::create_dir_all("target/m6-compare").expect("dir");
    let img = image::RgbaImage::from_fn(4, 4, |x, _| {
        if x < 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        }
    });
    img.save(path).expect("write test image");
    path.to_string()
}

#[test]
fn sprites_and_atlas_frames_draw_from_their_image() {
    let Some(mut canvas) = offscreen() else {
        return;
    };
    let path = quadrant_png();
    let sprite = |x: f32, dest: Option<[f32; 2]>, source: Option<[f32; 4]>, tint: [f32; 4]| {
        Draw2d::Texture {
            path: path.clone(),
            x,
            y: 0.0,
            dest,
            source,
            tint,
        }
    };
    let frame = canvas
        .render(
            &[
                // Native size, scaled to 40×40, the right-half cell of a
                // 2×1 atlas scaled, and tinted.
                sprite(0.0, None, None, WHITE),
                sprite(100.0, Some([40.0, 40.0]), None, WHITE),
                sprite(200.0, Some([40.0, 40.0]), Some([2.0, 0.0, 2.0, 4.0]), WHITE),
                sprite(300.0, Some([40.0, 40.0]), None, [0.5, 0.5, 0.5, 1.0]),
            ],
            None,
        )
        .expect("render");
    assert_eq!(&px(&frame, 0, 1)[..3], &[255, 0, 0]);
    assert_eq!(&px(&frame, 3, 1)[..3], &[0, 0, 255]);
    assert_eq!(px(&frame, 5, 1)[2], 0, "native size is 4 px");
    assert_eq!(&px(&frame, 105, 20)[..3], &[255, 0, 0]);
    assert_eq!(&px(&frame, 135, 20)[..3], &[0, 0, 255]);
    assert_eq!(px(&frame, 141, 20)[2], 0, "scaled to 40 px");
    // Linear filtering blends across a cell's edge, as macroquad's did;
    // the cell's middle is pure.
    assert_eq!(
        &px(&frame, 220, 20)[..3],
        &[0, 0, 255],
        "the atlas cell is the blue half"
    );
    let tinted = px(&frame, 305, 20);
    assert!(
        (126..=128).contains(&tinted[0]) && tinted[2] == 0,
        "tint multiplies: {tinted:?}"
    );
}

#[test]
fn a_camera_moves_the_world_not_the_canvas() {
    let Some(mut canvas) = offscreen() else {
        return;
    };
    let cam = Camera2d {
        center: [1000.0, 1000.0],
        zoom: 2.0,
    };
    let frame = canvas
        .render(
            &[Draw2d::Rect {
                x: 995.0,
                y: 995.0,
                w: 10.0,
                h: 10.0,
                color: RED,
            }],
            Some(cam),
        )
        .expect("render");
    // World (1000, 1000) is the canvas centre; zoom 2 makes it 20 px.
    assert_eq!(px(&frame, 320, 240)[0], 255);
    assert_eq!(px(&frame, 311, 231)[0], 255);
    assert_eq!(px(&frame, 308, 240)[0], 0);
}

// --- the reference comparison -------------------------------------------

/// One example's frame, produced headless through `frame2d`.
fn headless_frame(path: &str, frame: u32) -> Result<(Vec<Draw2d>, Option<Camera2d>), String> {
    let mut env = twec::frame2d::load(path)?;
    let dt = twec::eval::PHYSICS_DT;
    let mut accumulator = 0.0;
    for n in 1..=frame {
        twec::frame2d::tick(&mut env, &mut accumulator, dt).map_err(|e| format!("tick: {e}"))?;
        let (camera, result) = twec::frame2d::render(&mut env, dt, W as f32, H as f32);
        result.map_err(|e| format!("render: {e}"))?;
        if n == frame {
            return Ok((std::mem::take(&mut env.draw2d), camera));
        }
    }
    unreachable!("frame >= 1")
}

struct Score {
    /// Mean absolute difference per channel, 0–255.
    mean: f64,
    /// Share of pixels where some channel differs by more than 32.
    differing: f64,
}

fn score(a: &[u8], b: &[u8]) -> Score {
    let (mut sum, mut differing) = (0u64, 0u64);
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
        sum += (0..3).map(|c| u64::from(p[c].abs_diff(q[c]))).sum::<u64>();
        if d > 32 {
            differing += 1;
        }
    }
    let n = (a.len() / 4) as f64;
    Score {
        mean: sum as f64 / (n * 3.0),
        differing: differing as f64 / n,
    }
}

fn example_path(name: &str) -> String {
    let file = format!("examples/{name}.twe");
    if std::path::Path::new(&file).exists() {
        file
    } else {
        format!("examples/{name}/main.twe")
    }
}

/// Examples the kernel can't draw yet: they use UI widgets or panels,
/// which arrive in M6 session 5. Every other example must match its
/// reference.
const PENDING: &[&str] = &[
    "button_demo",
    "keybind_demo",
    "layout_demo",
    "pause_demo",
    "pause_menu_demo",
    "widgets_demo",
];

#[test]
fn kernel_frames_match_the_macroquad_references() {
    let Some(mut canvas) = offscreen() else {
        return;
    };
    let out = std::path::Path::new("target/m6-compare");
    std::fs::create_dir_all(out).expect("output dir");
    let mut refs: Vec<_> = std::fs::read_dir("tests/references/m6")
        .expect("references")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    refs.sort();
    let mut report = String::from("| frame | mean diff | pixels off | note |\n|---|---|---|---|\n");
    let mut failures = Vec::new();
    for reference in refs {
        let stem = reference.file_stem().unwrap().to_string_lossy().to_string();
        let (name, frame) = stem.rsplit_once("-f").expect("name-fNNN");
        let frame: u32 = frame.parse().expect("frame number");
        let path = example_path(name);
        // `twec play` resolves assets from the script's directory too.
        let dir = std::path::Path::new(&path)
            .parent()
            .map(|d| d.to_path_buf());
        twec::bundle::set_asset_root(dir);
        // A fresh thread per example: the runtime's thread-locals (fx,
        // light2d, camera shake, pause) start clean, as in a new process.
        let run = std::thread::spawn(move || headless_frame(&path, frame))
            .join()
            .unwrap_or_else(|_| Err("panicked".to_string()));
        let (items, camera) = match run {
            Ok(r) => r,
            Err(e) => {
                let note = e.lines().next().unwrap_or("").replace('|', "/");
                report.push_str(&format!("| {stem} | – | – | {note} |\n"));
                if !PENDING.contains(&name) {
                    failures.push(format!("{stem}: {note}"));
                }
                continue;
            }
        };
        let kernel = canvas.render(&items, camera).expect("render");
        image::save_buffer(
            out.join(format!("{stem}.png")),
            &kernel,
            W,
            H,
            image::ColorType::Rgba8,
        )
        .expect("write png");
        let reference = image::open(&reference).expect("reference png").to_rgba8();
        let s = score(&kernel, reference.as_raw());
        let ok = s.mean < 2.0 && s.differing < 0.01;
        report.push_str(&format!(
            "| {stem} | {:.2} | {:.2}% | {} |\n",
            s.mean,
            s.differing * 100.0,
            if ok { "" } else { "over threshold" }
        ));
        if !PENDING.contains(&name) && !ok {
            failures.push(format!(
                "{stem}: mean {:.2}, {:.2}% off",
                s.mean,
                s.differing * 100.0
            ));
        }
    }
    twec::bundle::set_asset_root(None);
    std::fs::write(out.join("report.md"), &report).expect("write report");
    println!("{report}");
    assert!(
        failures.is_empty(),
        "frames off their reference:\n{}",
        failures.join("\n")
    );
}
