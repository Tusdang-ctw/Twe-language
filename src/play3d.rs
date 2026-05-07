//! `twec play3d` — wgpu-driven 3D backend.
//!
//! Phase 5 task 5 closed at v0.1-minimum-viable on 2026-04-29 across
//! six sessions, all landing here:
//!
//! - **(a)** Clear-color window via wgpu + winit
//!   (`docs/changes/2026-04-29-phase-5-task-5-session-1-wgpu-scaffold.md`).
//! - **(b) + (c)** Vertex / index / camera buffers, WGSL flat-shading
//!   pipeline, depth buffer, hand-rolled column-major matrix math
//!   (`docs/changes/2026-04-29-phase-5-task-5-sessions-bc-cube-and-camera.md`).
//! - **(d) + (e)** Twe-driven scene: top-level `on render():` queues
//!   cubes via `cube(at:, color:, size:)`; `camera.eye`/`.target`/`.up`
//!   are mutable ambient fields. `vec3(x, y, z)` constructor + math
//!   primitives. One instanced draw call per frame, up to 4096 cubes
//!   (`docs/changes/2026-04-29-phase-5-task-5-sessions-de-twe-driven-3d.md`).
//! - **Carry-over** (this module's final shape): winit `KeyboardInput`
//!   → Twe `key.*` / `key_press.*`, mtime-poll hot reload, per-vertex
//!   normals + Lambertian directional shading. `tick_frame` runs the
//!   script's `on update(dt):` before each render so input-driven
//!   logic actually fires.
//!
//! Architecture matches `src/play.rs`'s split — startup runs the
//! script's top-level once (registering globals + the `on update`
//! / `on render` handlers), then the platform render loop drives
//! per-frame invocation. Each `RedrawRequested` does, in order:
//! hot-reload poll, key-state push into env, `tick_frame` (which
//! fires `on update(dt):`), `render_frame3d` (which fires
//! `on render():` and drains the cube queue), GPU submit, present.
//!
//! v0.2 task-5 follow-ons (per `docs/changes/2026-04-29-phase-5-closeout.md`):
//! `.glb` / `.obj` mesh import (session 1, this module's `mesh()` plus
//! `load_glb`), generic primitives (`sphere`, `plane`, `mesh`), bytecode
//! VM 3D path, mouse input, proper lighting (point / area / shadows),
//! `mat4` / `quat` stdlib types.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::value::{DrawCall3d, Env, Object, Primitive, Value};
use crate::{eval, lexer, parser, stdlib};

/// Twe-side key names ↔ winit physical key codes. Same name set
/// `src/play.rs` exposes for the macroquad path; the user's
/// `key.right` reads the same way no matter which loop is driving.
const KEYS: &[(&str, KeyCode)] = &[
    ("right", KeyCode::ArrowRight),
    ("left", KeyCode::ArrowLeft),
    ("up", KeyCode::ArrowUp),
    ("down", KeyCode::ArrowDown),
    ("space", KeyCode::Space),
    ("escape", KeyCode::Escape),
    ("enter", KeyCode::Enter),
    ("r", KeyCode::KeyR),
    ("w", KeyCode::KeyW),
    ("a", KeyCode::KeyA),
    ("s", KeyCode::KeyS),
    ("d", KeyCode::KeyD),
];

/// Mouse-button names exposed to Twe code. Same set the macroquad
/// path uses. v0.2 session 3.
const MOUSE_BUTTON_NAMES: &[&str] = &["left", "middle", "right"];

/// Map a winit `MouseButton` to its Twe-side name. Buttons beyond
/// left / middle / right (Back / Forward / Other) aren't surfaced
/// in v0.2 — match what macroquad exposes for cross-backend parity.
fn mouse_button_name(b: MouseButton) -> Option<&'static str> {
    match b {
        MouseButton::Left => Some("left"),
        MouseButton::Middle => Some("middle"),
        MouseButton::Right => Some("right"),
        _ => None,
    }
}

/// Cap on instances per frame. Keeps the per-instance buffer at a
/// fixed size — no reallocation on the hot path. 4096 cubes is
/// well past what a single Twe scene can usefully push at 60fps;
/// raise if the bottleneck moves elsewhere.
const MAX_INSTANCES: u64 = 4096;

/// `twec play3d <file>` entry. Parses + runs the file's top-level
/// code once, then enters the wgpu render loop until the window
/// closes. Returns the process exit code.
pub fn launch(path: String) -> i32 {
    let env = match initialize(&path) {
        Ok(env) => env,
        Err(()) => return 1,
    };
    let last_mtime = current_mtime(&path);

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("error: could not create event loop: {e}");
            return 1;
        }
    };
    let mut app = App::new(env, path, last_mtime);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("error: event loop: {e}");
        return 1;
    }
    app.exit_code
}

fn current_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(Path::new(path))
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Lex + parse + run the Twe file's top-level statements once. Any
/// error during this phase prints the diagnostic and returns Err so
/// the caller doesn't open a window. Returns the live Env for the
/// render loop to call `on render():` against on every frame.
fn initialize(path: &str) -> Result<Env, ()> {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read {path}: {e}");
            return Err(());
        }
    };
    let tokens = match lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{}:{}: {}", e.line, e.col, e.message);
            return Err(());
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{}:{}: {}", e.line, e.col, e.message);
            if let Some(help) = &e.help {
                eprintln!("  help: {help}");
            }
            return Err(());
        }
    };
    let mut env = Env::new();
    stdlib::install(&mut env);
    if let Err(e) = eval::run_top_level(&mut env, &program) {
        eprintln!("{path}:{}:{}: {}", e.line, e.col, e.message);
        if let Some(help) = &e.help {
            eprintln!("  help: {help}");
        }
        return Err(());
    }
    if !env.out.is_empty() {
        // Drain any startup `print` output to stdout so the user
        // sees it before the window opens.
        print!("{}", env.out);
        env.out.clear();
    }
    Ok(env)
}

// ---------- Vertex / instance / uniform layouts ----------

/// Per-vertex data — one upload at startup, never changes. The unit
/// cube's twenty-four vertices (four per face) live here; the
/// per-face normal drives Lambertian shading in the fragment
/// stage, so each face shades according to its angle to the
/// directional light.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    /// Model-space position in [-0.5, 0.5]³ (a unit cube centered
    /// at origin). The vertex shader translates and scales by the
    /// instance.
    position: [f32; 3],
    /// Per-face outward normal. Same value for all four corners
    /// of a face. Used for Lambertian diffuse shading.
    normal: [f32; 3],
    /// Texture coordinate. Phase 17 session 2: cube/sphere ship
    /// `[0.0, 0.0]` because they don't carry meaningful UVs (the
    /// fallback white texture sampled at any uv produces the same
    /// pixel). glb-loaded meshes write the `TEXCOORD_0` accessor
    /// here when present, `[0.0, 0.0]` otherwise.
    uv: [f32; 2],
}

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        4 => Float32x2,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Per-instance data — written once per frame from `env.render_queue3d`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Instance {
    position: [f32; 3],
    size: f32,
    color: [f32; 4],
}

impl Instance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
        2 => Float32x4, // packed (position.xyz, size)
        3 => Float32x4,
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Per-frame camera uniform.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}

const SHADER_SRC: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Phase 17 session 3: texture sampler. Bound per draw call by
// the play loop — fallback white 1x1 texture is bound when the
// mesh has no texture, so untextured rendering still works.
@group(1) @binding(0) var t_diffuse: texture_2d<f32>;
@group(1) @binding(1) var s_diffuse: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(4) uv: vec2<f32>,
};

struct InstanceInput {
    @location(2) inst_pos_size: vec4<f32>, // xyz = position, w = uniform scale
    @location(3) inst_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) base_color: vec3<f32>,
    @location(2) tex_coord: vec2<f32>,
};

// Hardcoded sun direction (towards the light) and intensities.
// The shading model is Lambertian + a constant ambient floor so
// faces away from the sun aren't pure black. When session (e+1)
// brings lights into Twe, these constants become uniform-buffer
// fields the script can write.
const SUN_DIR: vec3<f32> = vec3<f32>(0.4, 0.85, 0.35);
const AMBIENT: f32 = 0.30;

@vertex
fn vs_main(vert: VertexInput, inst: InstanceInput) -> VertexOutput {
    let model_pos = vert.position * inst.inst_pos_size.w + inst.inst_pos_size.xyz;
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(model_pos, 1.0);
    out.world_normal = vert.normal;
    out.base_color = inst.inst_color.rgb;
    out.tex_coord = vert.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(SUN_DIR);
    let diffuse = max(dot(n, l), 0.0);
    let lit = AMBIENT + diffuse * (1.0 - AMBIENT);
    // Sample the bound texture and modulate by the per-instance
    // color tint. For untextured meshes the fallback white texture
    // makes this a no-op (1.0 * tint) so existing scenes look the
    // same. Phase 17 session 3.
    let tex = textureSample(t_diffuse, s_diffuse, in.tex_coord);
    return vec4<f32>(in.base_color * tex.rgb * lit, tex.a);
}
"#;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

// ---------- Cube geometry ----------
//
// 24 vertices, 4 per face, so each face carries one outward
// normal uniformly. Indices wind CCW from outside; the pipeline's
// `Face::Back` cull drops interior triangles. The fragment shader
// computes Lambertian diffuse from these normals against a fixed
// directional light.

const N_FRONT: [f32; 3] = [0.0, 0.0, 1.0];
const N_BACK: [f32; 3] = [0.0, 0.0, -1.0];
const N_RIGHT: [f32; 3] = [1.0, 0.0, 0.0];
const N_LEFT: [f32; 3] = [-1.0, 0.0, 0.0];
const N_TOP: [f32; 3] = [0.0, 1.0, 0.0];
const N_BOTTOM: [f32; 3] = [0.0, -1.0, 0.0];

#[rustfmt::skip]
// Phase 17 session 2: per-face UVs run (0,0) at top-left to (1,1)
// at bottom-right of each face, so a 2x3 atlas of face textures
// could decorate the cube. For untextured cubes the fallback
// white texture means the value is irrelevant.
const UV_TL: [f32; 2] = [0.0, 0.0];
const UV_TR: [f32; 2] = [1.0, 0.0];
const UV_BR: [f32; 2] = [1.0, 1.0];
const UV_BL: [f32; 2] = [0.0, 1.0];

const CUBE_VERTICES: &[Vertex] = &[
    // +z (front)
    Vertex { position: [-0.5, -0.5,  0.5], normal: N_FRONT,  uv: UV_BL },
    Vertex { position: [ 0.5, -0.5,  0.5], normal: N_FRONT,  uv: UV_BR },
    Vertex { position: [ 0.5,  0.5,  0.5], normal: N_FRONT,  uv: UV_TR },
    Vertex { position: [-0.5,  0.5,  0.5], normal: N_FRONT,  uv: UV_TL },
    // -z (back)
    Vertex { position: [ 0.5, -0.5, -0.5], normal: N_BACK,   uv: UV_BL },
    Vertex { position: [-0.5, -0.5, -0.5], normal: N_BACK,   uv: UV_BR },
    Vertex { position: [-0.5,  0.5, -0.5], normal: N_BACK,   uv: UV_TR },
    Vertex { position: [ 0.5,  0.5, -0.5], normal: N_BACK,   uv: UV_TL },
    // +x (right)
    Vertex { position: [ 0.5, -0.5,  0.5], normal: N_RIGHT,  uv: UV_BL },
    Vertex { position: [ 0.5, -0.5, -0.5], normal: N_RIGHT,  uv: UV_BR },
    Vertex { position: [ 0.5,  0.5, -0.5], normal: N_RIGHT,  uv: UV_TR },
    Vertex { position: [ 0.5,  0.5,  0.5], normal: N_RIGHT,  uv: UV_TL },
    // -x (left)
    Vertex { position: [-0.5, -0.5, -0.5], normal: N_LEFT,   uv: UV_BL },
    Vertex { position: [-0.5, -0.5,  0.5], normal: N_LEFT,   uv: UV_BR },
    Vertex { position: [-0.5,  0.5,  0.5], normal: N_LEFT,   uv: UV_TR },
    Vertex { position: [-0.5,  0.5, -0.5], normal: N_LEFT,   uv: UV_TL },
    // +y (top)
    Vertex { position: [-0.5,  0.5,  0.5], normal: N_TOP,    uv: UV_BL },
    Vertex { position: [ 0.5,  0.5,  0.5], normal: N_TOP,    uv: UV_BR },
    Vertex { position: [ 0.5,  0.5, -0.5], normal: N_TOP,    uv: UV_TR },
    Vertex { position: [-0.5,  0.5, -0.5], normal: N_TOP,    uv: UV_TL },
    // -y (bottom)
    Vertex { position: [-0.5, -0.5, -0.5], normal: N_BOTTOM, uv: UV_BL },
    Vertex { position: [ 0.5, -0.5, -0.5], normal: N_BOTTOM, uv: UV_BR },
    Vertex { position: [ 0.5, -0.5,  0.5], normal: N_BOTTOM, uv: UV_TR },
    Vertex { position: [-0.5, -0.5,  0.5], normal: N_BOTTOM, uv: UV_TL },
];

#[rustfmt::skip]
const CUBE_INDICES: &[u16] = &[
    0,  1,  2,    0,  2,  3,    // +z
    4,  5,  6,    4,  6,  7,    // -z
    8,  9,  10,   8,  10, 11,   // +x
    12, 13, 14,   12, 14, 15,   // -x
    16, 17, 18,   16, 18, 19,   // +y
    20, 21, 22,   20, 22, 23,   // -y
];

// ---------- Sphere geometry (Phase 6 session 7) ----------
//
// Procedural UV-sphere of radius 0.5 centred at the origin (so a
// `sphere(size: 1.0)` matches `cube(size: 1.0)` visually — both
// span [-0.5, 0.5] in their model-space bounding box). Generated
// at startup, uploaded once. CCW winding from outside so the
// existing back-face-cull pipeline drops interior triangles.
//
// Latitude × longitude segments chosen for "looks round at a few
// hundred pixels" — denser meshes are nice but cost vertex
// throughput. 16 × 24 = 384 vertices, 720 triangles. Indexed as
// u16, comfortably under the 65k cap.

const SPHERE_LAT_SEGMENTS: u32 = 16;
const SPHERE_LON_SEGMENTS: u32 = 24;

fn sphere_mesh() -> (Vec<Vertex>, Vec<u16>) {
    let lat = SPHERE_LAT_SEGMENTS;
    let lon = SPHERE_LON_SEGMENTS;
    let pi = std::f32::consts::PI;
    let mut vertices: Vec<Vertex> = Vec::with_capacity(((lat + 1) * (lon + 1)) as usize);
    for i in 0..=lat {
        let v = i as f32 / lat as f32;
        let theta = v * pi; // [0, π] from +y down to -y
        let sin_t = theta.sin();
        let cos_t = theta.cos();
        for j in 0..=lon {
            let u = j as f32 / lon as f32;
            let phi = u * 2.0 * pi; // [0, 2π] around y axis
            let sin_p = phi.sin();
            let cos_p = phi.cos();
            // Unit-sphere position; normal is the same (radial).
            // Scale to radius 0.5 so the bounding box matches the
            // unit cube's [-0.5, 0.5]³.
            let nx = sin_t * cos_p;
            let ny = cos_t;
            let nz = sin_t * sin_p;
            vertices.push(Vertex {
                position: [nx * 0.5, ny * 0.5, nz * 0.5],
                normal: [nx, ny, nz],
                // Standard UV-sphere mapping: longitude → u, latitude → v.
                uv: [u, v],
            });
        }
    }
    let mut indices: Vec<u16> = Vec::with_capacity((lat * lon * 6) as usize);
    let stride = lon + 1;
    for i in 0..lat {
        for j in 0..lon {
            // Two triangles per quad. CCW from outside given the
            // (lat, lon) → (theta, phi) mapping above, where +y is
            // theta=0 (top of the sphere).
            let a = (i * stride + j) as u16;
            let b = (i * stride + j + 1) as u16;
            let c = ((i + 1) * stride + j + 1) as u16;
            let d = ((i + 1) * stride + j) as u16;
            indices.push(a);
            indices.push(d);
            indices.push(c);
            indices.push(a);
            indices.push(c);
            indices.push(b);
        }
    }
    (vertices, indices)
}

// ---------- App / state ----------

struct App {
    /// Live `Env` from `initialize`. Held across the event loop so
    /// the per-frame `on render():` invocation reaches the same
    /// globals the script registered at startup. Replaced wholesale
    /// on hot reload.
    env: Env,
    state: Option<RenderState>,
    last_frame_at: Instant,
    /// Source path + last-seen mtime for hot reload polling.
    path: String,
    last_mtime: Option<SystemTime>,
    /// Currently-held physical keys — Twe `key.<name>` reads this.
    /// Updated on every winit `KeyboardInput` event with state
    /// `Pressed` / `Released`.
    keys_held: HashSet<&'static str>,
    /// Keys whose `Pressed` event arrived since the last frame —
    /// drained into Twe `key_press.<name>` once per frame and
    /// cleared. Matches the macroquad path's edge-triggered
    /// semantics.
    keys_pressed_this_frame: HashSet<&'static str>,
    /// Mouse cursor position in window logical pixels. Updated on
    /// `WindowEvent::CursorMoved`. v0.2 session 3.
    mouse_x: f64,
    mouse_y: f64,
    /// Raw mouse delta accumulated since the last frame, in winit
    /// device-relative units (NOT logical pixels — driver-defined).
    /// Phase 17 session 3: lets a FPS camera read `mouse.dx`/`mouse.dy`
    /// without sensitivity-killing cursor wraparound. Reset to 0 each
    /// frame after the env update.
    mouse_dx: f64,
    mouse_dy: f64,
    /// Wheel delta accumulated this frame (line-delta y, unitless
    /// scroll-tick count for typical mice). Reset each frame after
    /// the env update.
    mouse_wheel_y: f32,
    /// Mouse buttons currently held — Twe `mouse_held.<name>`.
    mouse_buttons_held: HashSet<&'static str>,
    /// Mouse buttons whose `Pressed` event arrived since the last
    /// frame — Twe `mouse_press.<name>`. Edge-triggered.
    mouse_buttons_pressed_this_frame: HashSet<&'static str>,
    exit_code: i32,
}

impl App {
    fn new(env: Env, path: String, last_mtime: Option<SystemTime>) -> Self {
        Self {
            env,
            state: None,
            last_frame_at: Instant::now(),
            path,
            last_mtime,
            keys_held: HashSet::new(),
            keys_pressed_this_frame: HashSet::new(),
            mouse_x: 0.0,
            mouse_y: 0.0,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            mouse_wheel_y: 0.0,
            mouse_buttons_held: HashSet::new(),
            mouse_buttons_pressed_this_frame: HashSet::new(),
            exit_code: 0,
        }
    }

    /// Map a winit physical-key code to the Twe-side `&'static str`
    /// name (`"right"`, `"space"`, …). Returns `None` for keys we
    /// don't surface — the input set matches the macroquad path's
    /// `KEYS` table so scripts behave the same on both backends.
    fn key_name(code: KeyCode) -> Option<&'static str> {
        KEYS.iter()
            .find_map(|(name, c)| if *c == code { Some(*name) } else { None })
    }
}

struct RenderState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    cube_vertex_buffer: wgpu::Buffer,
    cube_index_buffer: wgpu::Buffer,
    cube_index_count: u32,
    sphere_vertex_buffer: wgpu::Buffer,
    sphere_index_buffer: wgpu::Buffer,
    sphere_index_count: u32,
    instance_buffer: wgpu::Buffer,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_view: wgpu::TextureView,
    /// Lazy-loaded `.glb` mesh GPU resources, keyed by the
    /// `Env::mesh_paths` interned id (the `u32` payload of
    /// `Primitive::Mesh`). Populated on first sight of a new id in
    /// `env.render_queue3d`. v0.2 session 1.
    mesh_cache: HashMap<u32, GpuMesh>,
    /// Ids whose load already failed once. Skip the file I/O and
    /// the stderr noise on every subsequent frame; the user can
    /// fix the path and hot-reload to retry.
    mesh_load_failures: HashSet<u32>,
    /// Phase 17 session 3: texture bind group layout (reused for
    /// every per-texture bind group), default linear sampler, and
    /// fallback white 1×1 texture's bind group. Untextured meshes
    /// bind `white_bind_group` so the fragment shader's
    /// `textureSample` call always has something to sample.
    texture_bgl: wgpu::BindGroupLayout,
    default_sampler: wgpu::Sampler,
    white_bind_group: wgpu::BindGroup,
    /// Lazy-loaded textures keyed by `Env::texture_paths` interned id.
    /// Each entry is the bind group ready to set on render group 1.
    texture_cache: HashMap<u32, wgpu::BindGroup>,
    texture_load_failures: HashSet<u32>,
}

/// Per-mesh GPU buffers loaded from a `.glb` file. v0.2 session 1.
/// Stored in `RenderState::mesh_cache` keyed by the
/// `Env::mesh_paths` interned id.
struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    /// `.glb` accessors can use u8 / u16 / u32; we widen everything
    /// to u32 on load so the pipeline only needs one branch.
    index_format: wgpu::IndexFormat,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("Twec play3d")
            .with_inner_size(LogicalSize::new(640.0, 480.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("error: window create: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        match init_wgpu(window.clone()) {
            Ok(state) => self.state = Some(state),
            Err(e) => {
                eprintln!("error: wgpu init: {e}");
                self.exit_code = 1;
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let state = match self.state.as_mut() {
            Some(s) => s,
            None => return,
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    state.config.width = size.width;
                    state.config.height = size.height;
                    state.surface.configure(&state.device, &state.config);
                    state.depth_view =
                        create_depth_view(&state.device, state.config.width, state.config.height);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: key_state,
                        repeat,
                        ..
                    },
                ..
            } => {
                if let Some(name) = Self::key_name(code) {
                    match key_state {
                        ElementState::Pressed => {
                            self.keys_held.insert(name);
                            // Edge-triggered: only count the first
                            // `Pressed` event in a held sequence as
                            // a "press." OS auto-repeat fires
                            // additional `Pressed` events with
                            // `repeat = true` — drop those.
                            if !repeat {
                                self.keys_pressed_this_frame.insert(name);
                            }
                        }
                        ElementState::Released => {
                            self.keys_held.remove(name);
                        }
                    }
                }
                // Esc closes the window — same convention as the
                // macroquad path.
                if matches!(code, KeyCode::Escape) && key_state == ElementState::Pressed {
                    event_loop.exit();
                }
            }
            // v0.2 session 3: mouse events. CursorMoved tracks
            // position; MouseInput tracks button held + edge-press;
            // MouseWheel accumulates the per-frame wheel delta.
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_x = position.x;
                self.mouse_y = position.y;
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                if let Some(name) = mouse_button_name(button) {
                    match btn_state {
                        ElementState::Pressed => {
                            self.mouse_buttons_held.insert(name);
                            self.mouse_buttons_pressed_this_frame.insert(name);
                        }
                        ElementState::Released => {
                            self.mouse_buttons_held.remove(name);
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Two delta shapes: line-based (most desktop mice)
                // and pixel-based (trackpads). Normalize both into
                // a single y-axis scroll value summed for the frame.
                // Pixel-deltas tend to be ~10–20px per "tick"; the
                // 1/120 factor approximates the macroquad path's
                // tick-count semantics.
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_x, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y as f32) / 120.0,
                };
                self.mouse_wheel_y += dy;
            }
            WindowEvent::RedrawRequested => {
                // Hot reload: poll the source's mtime, re-init env
                // on change. Mirrors `src/play.rs::run_loop`. A
                // failed re-init keeps the current env so the
                // window doesn't tear down on a transient typo.
                let cur_mtime = current_mtime(&self.path);
                if cur_mtime.is_some() && cur_mtime != self.last_mtime {
                    if let Ok(new_env) = initialize(&self.path) {
                        eprintln!("[twec] hot reload: {}", self.path);
                        crate::stdlib::clear_asset_caches();
                        // The new env's `mesh_paths` indices are
                        // independent of the old env's, so cached
                        // GpuMesh entries by id are stale. Drop
                        // them and let the next frame re-load by
                        // path; on-disk `.glb` edits also pick up
                        // because of this.
                        state.mesh_cache.clear();
                        state.mesh_load_failures.clear();
                        state.texture_cache.clear();
                        state.texture_load_failures.clear();
                        // Phase 18: drop all rigid bodies; the new
                        // env will recreate them on its first
                        // `on update(dt)` tick. Otherwise stale
                        // handles from the prior run leak.
                        crate::physics3d::reset();
                        self.env = new_env;
                    }
                    // A failed re-init keeps the current env so the
                    // window doesn't tear down on a transient typo.
                    self.last_mtime = cur_mtime;
                }

                // Push input state into the Twe-visible `key` /
                // `key_press` Objects before running the frame.
                update_key_state(
                    &mut self.env,
                    &self.keys_held,
                    &self.keys_pressed_this_frame,
                );
                self.keys_pressed_this_frame.clear();
                // v0.2 session 3: same for mouse / mouse_held /
                // mouse_press. Wheel + edge-press are reset here.
                update_mouse_state(
                    &mut self.env,
                    self.mouse_x,
                    self.mouse_y,
                    self.mouse_dx,
                    self.mouse_dy,
                    self.mouse_wheel_y,
                    &self.mouse_buttons_held,
                    &self.mouse_buttons_pressed_this_frame,
                );
                self.mouse_wheel_y = 0.0;
                self.mouse_dx = 0.0;
                self.mouse_dy = 0.0;
                self.mouse_buttons_pressed_this_frame.clear();

                // Phase 17 session 3: drain any pending cursor-mode
                // request from the script side. cursor.lock() /
                // cursor.unlock() write a CursorMode here; we apply
                // it to the window once per frame so the request
                // takes effect even if the script is in a render
                // handler when it fires.
                if let Some(mode) = crate::stdlib::take_pending_cursor_mode() {
                    apply_cursor_mode(&state.window, mode);
                }

                let now = Instant::now();
                let dt = now.duration_since(self.last_frame_at).as_secs_f32();
                self.last_frame_at = now;
                if let Err(e) = render(state, &mut self.env, dt) {
                    eprintln!("render error: {e}");
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }

    /// Phase 17 session 3: raw mouse delta from the OS, independent
    /// of cursor position / wraparound. Required for FPS-style
    /// camera control.  `WindowEvent::CursorMoved` only gives
    /// absolute window coords, which jumps when the cursor is locked
    /// or wraps; `DeviceEvent::MouseMotion` is the raw integrated
    /// pointer velocity.
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.mouse_dx += dx;
            self.mouse_dy += dy;
        }
    }
}

/// Phase 17 session 3: apply a pending cursor-mode request from
/// the script. Locked grab is the FPS-style "infinite cursor"
/// mode; if the platform doesn't support Locked, we fall back to
/// Confined (which keeps the cursor inside the window). Visibility
/// is also toggled — locked games hide the cursor by convention.
fn apply_cursor_mode(window: &Window, locked: bool) {
    use winit::window::CursorGrabMode;
    if locked {
        // Try Locked first (raw input, no cursor movement). Some
        // platforms (older X11) only support Confined; fall back
        // gracefully rather than failing the call.
        if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        }
        window.set_cursor_visible(false);
    } else {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        window.set_cursor_visible(true);
    }
}

/// Write the current input state into the env's `key` and
/// `key_press` Objects so Twe scripts see `key.right`, etc.
/// Mirrors `src/play.rs::update_key_state` but reads from the
/// winit-fed `HashSet`s rather than macroquad's `is_key_down` /
/// `is_key_pressed`.
fn update_key_state(env: &mut Env, held: &HashSet<&'static str>, pressed: &HashSet<&'static str>) {
    if let Some(t) = env.get("key") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            for (name, _) in KEYS {
                o.insert_field(*name, Value::from_bool(held.contains(name)));
            }
        }
    }
    let kp = env.get("key_press");
    if kp.as_ref().is_some_and(|t| t.is_object()) {
        let rc = kp.unwrap().as_object();
        let mut o = rc.borrow_mut();
        for (name, _) in KEYS {
            o.insert_field(*name, Value::from_bool(pressed.contains(name)));
        }
    } else {
        let mut press = Object {
            fields: HashMap::new(),
            kind: "input",
        };
        for (name, _) in KEYS {
            press.insert_field(*name, Value::from_bool(pressed.contains(name)));
        }
        env.set(
            "key_press".to_string(),
            Value::from_object(Rc::new(RefCell::new(press))),
        );
    }
}

/// Write the current mouse state into the env's `mouse`,
/// `mouse_held`, and `mouse_press` Objects. Mirror of
/// `update_key_state` for cursor + buttons + wheel. v0.2 session 3.
#[allow(clippy::too_many_arguments)]
fn update_mouse_state(
    env: &mut Env,
    mouse_x: f64,
    mouse_y: f64,
    mouse_dx: f64,
    mouse_dy: f64,
    wheel_y: f32,
    held: &HashSet<&'static str>,
    pressed: &HashSet<&'static str>,
) {
    if let Some(t) = env.get("mouse") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            o.insert_field("x", Value::from_float(mouse_x));
            o.insert_field("y", Value::from_float(mouse_y));
            o.insert_field("dx", Value::from_float(mouse_dx));
            o.insert_field("dy", Value::from_float(mouse_dy));
            o.insert_field(
                "pos",
                Value::from_tuple(Rc::new(vec![
                    Value::from_float(mouse_x),
                    Value::from_float(mouse_y),
                ])),
            );
            o.insert_field("wheel", Value::from_float(wheel_y as f64));
        }
    }
    if let Some(t) = env.get("mouse_held") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            for name in MOUSE_BUTTON_NAMES {
                o.insert_field(*name, Value::from_bool(held.contains(name)));
            }
        }
    }
    if let Some(t) = env.get("mouse_press") {
        if t.is_object() {
            let rc = t.as_object();
            let mut o = rc.borrow_mut();
            for name in MOUSE_BUTTON_NAMES {
                o.insert_field(*name, Value::from_bool(pressed.contains(name)));
            }
        }
    }
}

// ---------- wgpu setup ----------

fn init_wgpu(window: Arc<Window>) -> Result<RenderState, String> {
    let size = window.inner_size();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let surface = instance
        .create_surface(window.clone())
        .map_err(|e| e.to_string())?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| "no compatible wgpu adapter found".to_string())?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("twec-play3d device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .map_err(|e| e.to_string())?;
    let surface_caps = surface.get_capabilities(&adapter);
    let surface_format = surface_caps
        .formats
        .iter()
        .find(|f| f.is_srgb())
        .copied()
        .unwrap_or_else(|| surface_caps.formats[0]);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: surface_caps.present_modes[0],
        alpha_mode: surface_caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    // Vertex + index buffers, uploaded once. Cube is a const,
    // sphere is generated at startup (the procedural mesh is
    // small and the cost amortises across the program's lifetime).
    let cube_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d cube vertices"),
        contents: bytemuck::cast_slice(CUBE_VERTICES),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let cube_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d cube indices"),
        contents: bytemuck::cast_slice(CUBE_INDICES),
        usage: wgpu::BufferUsages::INDEX,
    });
    let (sphere_verts, sphere_idxs) = sphere_mesh();
    let sphere_index_count = sphere_idxs.len() as u32;
    let sphere_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d sphere vertices"),
        contents: bytemuck::cast_slice(&sphere_verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let sphere_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d sphere indices"),
        contents: bytemuck::cast_slice(&sphere_idxs),
        usage: wgpu::BufferUsages::INDEX,
    });

    // Instance buffer — preallocated up to MAX_INSTANCES, written
    // each frame from `env.render_queue3d`.
    let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twec-play3d instances"),
        size: MAX_INSTANCES * std::mem::size_of::<Instance>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Camera uniform.
    let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twec-play3d camera"),
        size: std::mem::size_of::<CameraUniform>() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twec-play3d camera bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twec-play3d camera bg"),
        layout: &camera_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buffer.as_entire_binding(),
        }],
    });

    // Phase 17 session 3: texture bind group layout — sampled
    // 2D float texture + a filtering sampler. One layout, reused
    // for every per-mesh texture binding (including the fallback
    // white texture used by untextured meshes).
    let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twec-play3d texture bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    // White 1x1 fallback texture so untextured meshes draw correctly
    // (sampling any uv produces white, which multiplied by the
    // per-instance tint gives the unmodified tint).
    let white_texture = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: Some("twec-play3d white fallback"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::default(),
        &[255, 255, 255, 255],
    );
    let white_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let default_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("twec-play3d sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let white_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twec-play3d white bg"),
        layout: &texture_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&white_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&default_sampler),
            },
        ],
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("twec-play3d shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("twec-play3d pipeline layout"),
        bind_group_layouts: &[&camera_bgl, &texture_bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("twec-play3d pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[Vertex::layout(), Instance::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let depth_view = create_depth_view(&device, config.width, config.height);

    Ok(RenderState {
        window,
        surface,
        device,
        queue,
        config,
        pipeline,
        cube_vertex_buffer,
        cube_index_buffer,
        cube_index_count: CUBE_INDICES.len() as u32,
        sphere_vertex_buffer,
        sphere_index_buffer,
        sphere_index_count,
        instance_buffer,
        camera_buffer,
        camera_bind_group,
        depth_view,
        mesh_cache: HashMap::new(),
        mesh_load_failures: HashSet::new(),
        texture_bgl,
        default_sampler,
        white_bind_group,
        texture_cache: HashMap::new(),
        texture_load_failures: HashSet::new(),
    })
}

/// Phase 17 session 3: PNG/JPEG texture loader. Decodes via the
/// `image` crate, uploads as Rgba8UnormSrgb, returns a bind group
/// ready to set on render group 1. Path is resolved through the
/// bundle-aware loader so built `.exe`s work.
fn load_and_upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    path: &str,
) -> Result<wgpu::BindGroup, String> {
    let bytes = crate::bundle::read_asset_bytes(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("twec-play3d texture"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4 * w),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twec-play3d texture bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    }))
}

fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("twec-play3d depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

// ---------- glTF 2.0 mesh loader ----------
//
// v0.2 session 1. Pulls position + normal + indices out of the first
// primitive of the first mesh in a `.glb`. Returns CPU-side data the
// caller uploads to GPU. Multi-primitive scenes, node transforms,
// materials, and textures are all follow-ons — design notes in
// `notes/future-phases.md` "Carried into v0.2".

/// Decode a `.glb` (or `.gltf`) at `path`. Returns interleaved
/// `Vertex` array + u32 index list. Errors are stringified at the
/// boundary because the upstream `gltf::Error` carries lifetimes
/// we don't want to leak.
fn load_glb(path: &str) -> Result<(Vec<Vertex>, Vec<u32>), String> {
    // Phase 12 session 3: bundle-first lookup, filesystem fallback.
    let bytes = crate::bundle::read_asset_bytes(path).map_err(|e| e.to_string())?;
    parse_glb_bytes(&bytes)
}

/// Inner loader exposed for tests — drives the gltf crate against
/// an in-memory byte slice instead of a path so we can exercise
/// the decode path without shipping binary fixtures.
fn parse_glb_bytes(bytes: &[u8]) -> Result<(Vec<Vertex>, Vec<u32>), String> {
    let (doc, buffers, _images) = gltf::import_slice(bytes).map_err(|e| e.to_string())?;
    let mesh = doc
        .meshes()
        .next()
        .ok_or_else(|| "glb has no meshes".to_string())?;
    let primitive = mesh
        .primitives()
        .next()
        .ok_or_else(|| "first mesh has no primitives".to_string())?;
    let reader = primitive.reader(|b| Some(&buffers[b.index()]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| "primitive has no POSITION accessor".to_string())?
        .collect();
    if positions.is_empty() {
        return Err("primitive has zero vertices".to_string());
    }

    // Normals are optional in glTF. When absent, we fall back to a
    // constant up-vector — the mesh will shade flat-bright but at
    // least it draws. Computing flat normals from indices is a
    // follow-on; users who want shading should export with normals.
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(iter) => {
            let v: Vec<[f32; 3]> = iter.collect();
            if v.len() == positions.len() {
                v
            } else {
                vec![[0.0, 1.0, 0.0]; positions.len()]
            }
        }
        None => vec![[0.0, 1.0, 0.0]; positions.len()],
    };

    // Phase 17 session 2: read TEXCOORD_0 if present. Most Blender /
    // Mixamo exports include it. Falls back to (0, 0) per vertex —
    // sampling the fallback white texture at any uv produces white,
    // so untextured meshes still render correctly.
    let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
        Some(iter) => {
            let v: Vec<[f32; 2]> = iter.into_f32().collect();
            if v.len() == positions.len() {
                v
            } else {
                vec![[0.0, 0.0]; positions.len()]
            }
        }
        None => vec![[0.0, 0.0]; positions.len()],
    };

    let vertices: Vec<Vertex> = positions
        .iter()
        .zip(normals.iter())
        .zip(uvs.iter())
        .map(|((p, n), uv)| Vertex {
            position: *p,
            normal: *n,
            uv: *uv,
        })
        .collect();

    // Indices are optional too — non-indexed primitives implicitly
    // index 0..N. `into_u32()` widens whatever the file used (u8 /
    // u16 / u32) so we only need one GPU index format.
    let indices: Vec<u32> = match reader.read_indices() {
        Some(idx) => idx.into_u32().collect(),
        None => (0..vertices.len() as u32).collect(),
    };

    Ok((vertices, indices))
}

/// Load `.glb` at `path`, upload CPU-side data to GPU, return a
/// `GpuMesh` ready for rendering. Logs the failure path to stderr
/// on any error so the user sees what went wrong.
fn load_and_upload_mesh(device: &wgpu::Device, path: &str) -> Result<GpuMesh, String> {
    let (vertices, indices) = load_glb(path)?;
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d mesh vertices"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("twec-play3d mesh indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    Ok(GpuMesh {
        vertex_buffer,
        index_buffer,
        index_count: indices.len() as u32,
        index_format: wgpu::IndexFormat::Uint32,
    })
}

// ---------- Per-frame render ----------

fn render(state: &mut RenderState, env: &mut Env, dt: f32) -> Result<(), String> {
    // Phase 18: step the rapier3d world before the Twe `on update`
    // body runs, so script logic reads authoritative positions.
    // Scripts own intent (velocity / impulse), the integrator owns
    // truth. No-op if the script never created any bodies — the
    // world's empty body set means step() returns near-instantly.
    crate::physics3d::step(dt);

    // 1. Tick the per-frame logic — top-level `on update(dt):`
    //    plus any active scene / entity tick. The macroquad path
    //    does the same in `src/play.rs::run_loop`, separating
    //    "advance simulation" (tick_frame) from "compose this
    //    frame" (render_frame3d). Without this the script's
    //    `on update(dt):` never fires, so anything that reads
    //    `key.*` to drive state stays frozen.
    if let Err(e) = eval::tick_frame(env, dt as f64) {
        eprintln!(
            "render error in `on update(dt)`: {}:{}: {}",
            e.line, e.col, e.message
        );
    }
    if !env.out.is_empty() {
        print!("{}", env.out);
        env.out.clear();
    }

    // 2. Run the script's `on render():` body. It pushes cubes
    //    onto `env.render_queue3d` (cleared by `render_frame3d`
    //    before the body runs).
    if let Err(e) = eval::render_frame3d(env) {
        // Surface the runtime error to stderr but keep rendering —
        // a broken render frame shouldn't tear down the window.
        eprintln!(
            "render error in `on render()`: {}:{}: {}",
            e.line, e.col, e.message
        );
    }
    if !env.out.is_empty() {
        // Drain any `print` output the body produced this frame.
        print!("{}", env.out);
        env.out.clear();
    }

    // 3. Snapshot the camera fields. The script mutates them via
    //    `camera.eye = vec3(...)`; missing fields fall back to
    //    sensible defaults so a script that never touches the
    //    camera still gets a viewable scene.
    let (eye, target, up) = read_camera(env);
    let aspect = state.config.width as f32 / state.config.height.max(1) as f32;
    let proj = perspective(60_f32.to_radians(), aspect, 0.1, 100.0);
    let view = look_at(eye, target, up);
    let view_proj = mul(proj, view);
    let camera_uniform = CameraUniform { view_proj };
    state
        .queue
        .write_buffer(&state.camera_buffer, 0, bytemuck::bytes_of(&camera_uniform));

    // 4. Partition the queue per primitive. Each primitive becomes
    //    one instanced draw call, packed contiguously into the
    //    shared instance buffer with a remembered (offset, count)
    //    so the draw call knows its slice. Drain the queue
    //    here — `render_frame3d` re-clears next frame, but
    //    draining now keeps the contract clean.
    let queue: Vec<DrawCall3d> = env.render_queue3d.drain(..).collect();
    let mut instances: Vec<Instance> = Vec::with_capacity(queue.len().min(MAX_INSTANCES as usize));
    let cap = MAX_INSTANCES as usize;

    // 4a. Lazy-load any `.glb` paths referenced this frame but not
    //     yet on the GPU. v0.2 session 1.
    let mut needed_mesh_ids: Vec<u32> = Vec::new();
    for d in &queue {
        if let Primitive::Mesh(id) = d.primitive {
            if !state.mesh_cache.contains_key(&id)
                && !state.mesh_load_failures.contains(&id)
                && !needed_mesh_ids.contains(&id)
            {
                needed_mesh_ids.push(id);
            }
        }
    }
    for id in needed_mesh_ids {
        let path = match env.mesh_path(id) {
            Some(p) => p.to_string(),
            None => {
                // Stale id — env was hot-reloaded between
                // `mesh()` and the render. Treat as a load failure
                // so we don't keep retrying.
                state.mesh_load_failures.insert(id);
                continue;
            }
        };
        match load_and_upload_mesh(&state.device, &path) {
            Ok(gpu_mesh) => {
                state.mesh_cache.insert(id, gpu_mesh);
            }
            Err(e) => {
                eprintln!("error: mesh load `{path}`: {e}");
                state.mesh_load_failures.insert(id);
            }
        }
    }

    // 4b. Phase 17 session 3: lazy-load any texture paths referenced
    //     this frame but not yet on the GPU. Same pattern as the
    //     mesh cache above.
    let mut needed_texture_ids: Vec<u32> = Vec::new();
    for d in &queue {
        let id = d.texture;
        if id != 0
            && !state.texture_cache.contains_key(&id)
            && !state.texture_load_failures.contains(&id)
            && !needed_texture_ids.contains(&id)
        {
            needed_texture_ids.push(id);
        }
    }
    for id in needed_texture_ids {
        let path = match env.texture_path(id) {
            Some(p) => p.to_string(),
            None => {
                state.texture_load_failures.insert(id);
                continue;
            }
        };
        match load_and_upload_texture(
            &state.device,
            &state.queue,
            &state.texture_bgl,
            &state.default_sampler,
            &path,
        ) {
            Ok(bg) => {
                state.texture_cache.insert(id, bg);
            }
            Err(e) => {
                eprintln!("error: texture load `{path}`: {e}");
                state.texture_load_failures.insert(id);
            }
        }
    }

    // Phase 17 session 3: group draws by (primitive, texture_id).
    // Each unique combination becomes its own instanced draw call
    // because group 1's bind group changes between textures.
    // Within a group, instance order = queue order (preserves any
    // back-to-front ordering the script established).
    let mut cube_groups: Vec<(u32, Vec<&DrawCall3d>)> = Vec::new();
    let mut sphere_groups: Vec<(u32, Vec<&DrawCall3d>)> = Vec::new();
    let mut mesh_groups: Vec<((u32, u32), Vec<&DrawCall3d>)> = Vec::new();
    for d in &queue {
        match d.primitive {
            Primitive::Cube => match cube_groups.iter_mut().find(|(t, _)| *t == d.texture) {
                Some((_, list)) => list.push(d),
                None => cube_groups.push((d.texture, vec![d])),
            },
            Primitive::Sphere => match sphere_groups.iter_mut().find(|(t, _)| *t == d.texture) {
                Some((_, list)) => list.push(d),
                None => sphere_groups.push((d.texture, vec![d])),
            },
            Primitive::Mesh(id) => {
                if !state.mesh_cache.contains_key(&id) {
                    continue;
                }
                let key = (id, d.texture);
                match mesh_groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, list)) => list.push(d),
                    None => mesh_groups.push((key, vec![d])),
                }
            }
        }
    }

    let push_group = |group: &[&DrawCall3d], out: &mut Vec<Instance>| -> (u32, u32) {
        let start = out.len() as u32;
        for d in group {
            if out.len() >= cap {
                break;
            }
            out.push(Instance {
                position: d.at,
                size: d.size,
                color: d.color,
            });
        }
        let end = out.len() as u32;
        (start, end)
    };
    let cube_ranges: Vec<(u32, (u32, u32))> = cube_groups
        .iter()
        .map(|(t, list)| (*t, push_group(list, &mut instances)))
        .collect();
    let sphere_ranges: Vec<(u32, (u32, u32))> = sphere_groups
        .iter()
        .map(|(t, list)| (*t, push_group(list, &mut instances)))
        .collect();
    let mesh_ranges: Vec<((u32, u32), (u32, u32))> = mesh_groups
        .iter()
        .map(|(k, list)| (*k, push_group(list, &mut instances)))
        .collect();
    if !instances.is_empty() {
        state
            .queue
            .write_buffer(&state.instance_buffer, 0, bytemuck::cast_slice(&instances));
    }

    // 5. Acquire the swapchain texture and draw.
    let frame = state
        .surface
        .get_current_texture()
        .map_err(|e| format!("acquire surface: {e}"))?;
    let view_target = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("twec-play3d encoder"),
        });
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("twec-play3d main pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view_target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.06,
                        g: 0.10,
                        b: 0.16,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &state.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if !instances.is_empty() {
            rpass.set_pipeline(&state.pipeline);
            rpass.set_bind_group(0, &state.camera_bind_group, &[]);
            rpass.set_vertex_buffer(1, state.instance_buffer.slice(..));
            // Phase 17 session 3: helper closure that picks the
            // right texture bind group for a given texture id.
            // 0 = white fallback; loaded ids look up texture_cache;
            // missing/failed ids fall through to white.
            let bind_for = |tex_id: u32| -> &wgpu::BindGroup {
                if tex_id == 0 {
                    return &state.white_bind_group;
                }
                state
                    .texture_cache
                    .get(&tex_id)
                    .unwrap_or(&state.white_bind_group)
            };
            // Cube draws — one per (texture) group.
            for (tex, range) in &cube_ranges {
                if range.1 <= range.0 {
                    continue;
                }
                rpass.set_bind_group(1, bind_for(*tex), &[]);
                rpass.set_vertex_buffer(0, state.cube_vertex_buffer.slice(..));
                rpass.set_index_buffer(
                    state.cube_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                rpass.draw_indexed(0..state.cube_index_count, 0, range.0..range.1);
            }
            // Sphere draws — one per (texture) group.
            for (tex, range) in &sphere_ranges {
                if range.1 <= range.0 {
                    continue;
                }
                rpass.set_bind_group(1, bind_for(*tex), &[]);
                rpass.set_vertex_buffer(0, state.sphere_vertex_buffer.slice(..));
                rpass.set_index_buffer(
                    state.sphere_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                rpass.draw_indexed(
                    0..state.sphere_index_count,
                    0,
                    range.0..range.1,
                );
            }
            // Mesh draws — one per (mesh id, texture) group. Each
            // unique combination is its own instanced draw call.
            for ((mesh_id, tex), range) in &mesh_ranges {
                if range.1 <= range.0 {
                    continue;
                }
                let gpu_mesh = match state.mesh_cache.get(mesh_id) {
                    Some(m) => m,
                    None => continue,
                };
                rpass.set_bind_group(1, bind_for(*tex), &[]);
                rpass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                rpass.set_index_buffer(gpu_mesh.index_buffer.slice(..), gpu_mesh.index_format);
                rpass.draw_indexed(0..gpu_mesh.index_count, 0, range.0..range.1);
            }
        }
    }
    state.queue.submit(Some(encoder.finish()));
    frame.present();
    Ok(())
}

/// Extract `camera.eye` / `camera.target` / `camera.up` from the
/// env's `camera` Object. Missing or malformed fields fall back to
/// the stdlib defaults (eye 3 units back + 1.5 up, looking at the
/// origin, +y up). Phase 5 task 5 session (d).
fn read_camera(env: &Env) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let eye_default = [0.0, 1.5, 3.0];
    let target_default = [0.0, 0.0, 0.0];
    let up_default = [0.0, 1.0, 0.0];
    let camera = {
        let __opt = env.get("camera");
        if let Some(__t) = (__opt).as_ref() {
            if __t.is_object() {
                let rc = __t.as_object();
                rc.clone()
            } else {
                return (eye_default, target_default, up_default);
            }
        } else {
            return (eye_default, target_default, up_default);
        }
    };
    let cam = camera.borrow();
    let eye = cam
        .get_field("eye")
        .as_ref()
        .and_then(value_as_vec3)
        .unwrap_or(eye_default);
    let target = cam
        .get_field("target")
        .as_ref()
        .and_then(value_as_vec3)
        .unwrap_or(target_default);
    let up = cam
        .get_field("up")
        .as_ref()
        .and_then(value_as_vec3)
        .unwrap_or(up_default);
    (eye, target, up)
}

fn value_as_vec3(v: &Value) -> Option<[f32; 3]> {
    if v.is_tuple() {
        let elems = v.as_tuple();
        if elems.len() == 3 {
            let x = number(&elems[0])?;
            let y = number(&elems[1])?;
            let z = number(&elems[2])?;
            return Some([x as f32, y as f32, z as f32]);
        }
    }
    None
}

fn number(v: &Value) -> Option<f64> {
    if v.is_int_or_boxed_int() {
        let n = v.as_int();
        Some(n as f64)
    } else if v.is_float() {
        let f = v.as_float();
        Some(f)
    } else {
        None
    }
}

// ---------- Hand-rolled column-major matrix math ----------

fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, (near * far) / (near - far), 0.0],
    ]
}

fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k][row] * b[col][k];
            }
            out[col][row] = sum;
        }
    }
    out
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        v
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn approx_mat(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> bool {
        (0..4).all(|c| (0..4).all(|r| approx(a[c][r], b[c][r])))
    }

    #[test]
    fn mul_identity_is_identity() {
        let id: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let p = perspective(60_f32.to_radians(), 1.0, 0.1, 100.0);
        assert!(approx_mat(mul(id, p), p));
        assert!(approx_mat(mul(p, id), p));
    }

    #[test]
    fn look_at_eye_at_origin_facing_minus_z_is_identity_axes() {
        let m = look_at([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        assert!(approx(m[0][0], 1.0));
        assert!(approx(m[1][1], 1.0));
        assert!(approx(m[2][2], 1.0));
        assert!(approx(m[3][3], 1.0));
    }

    #[test]
    fn perspective_maps_near_plane_to_zero_depth() {
        let near = 0.1;
        let p = perspective(60_f32.to_radians(), 1.0, near, 100.0);
        let point = [0.0, 0.0, -near, 1.0];
        let mut out = [0.0; 4];
        for r in 0..4 {
            for c in 0..4 {
                out[r] += p[c][r] * point[c];
            }
        }
        assert!(out[3] > 0.0);
        assert!(approx(out[2] / out[3], 0.0));
    }

    #[test]
    fn cross_basis_vectors() {
        // x × y = z
        let z = cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(approx(z[0], 0.0));
        assert!(approx(z[1], 0.0));
        assert!(approx(z[2], 1.0));
    }

    #[test]
    fn normalize_unit_vector_unchanged() {
        let v = normalize([0.0, 0.0, 1.0]);
        assert!(approx(v[2], 1.0));
    }

    #[test]
    fn normalize_zero_vector_safe() {
        // Don't divide by zero — return as-is rather than NaN.
        let v = normalize([0.0, 0.0, 0.0]);
        assert_eq!(v, [0.0, 0.0, 0.0]);
    }

    // ---------- v0.2 session 1: .glb loader ----------

    /// Build a minimal valid .glb in memory: one mesh, one
    /// primitive, three vertices forming a triangle, three u32
    /// indices, no normals (loader fills with up-vector). Used to
    /// exercise `parse_glb_bytes` without shipping binary fixtures.
    fn make_minimal_glb() -> Vec<u8> {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let indices: [u32; 3] = [0, 1, 2];
        let pos_bytes = bytemuck::cast_slice::<f32, u8>(&positions).to_vec();
        let idx_bytes = bytemuck::cast_slice::<u32, u8>(&indices).to_vec();
        let bin: Vec<u8> = [pos_bytes.as_slice(), idx_bytes.as_slice()].concat();

        // POSITION accessors require `min`/`max` per the glTF spec
        // (used by culling / bounds checks). For our triangle:
        // min = [0, 0, 0], max = [1, 1, 0].
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}},{{"bufferView":1,"componentType":5125,"count":3,"type":"SCALAR"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":12}}],"buffers":[{{"byteLength":{}}}]}}"#,
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        // glTF chunk data must be 4-byte aligned. JSON pads with
        // spaces (0x20), BIN pads with null bytes (0x00).
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut bin_bytes = bin;
        while !bin_bytes.len().is_multiple_of(4) {
            bin_bytes.push(0);
        }

        let total_len: u32 = 12 + 8 + json_bytes.len() as u32 + 8 + bin_bytes.len() as u32;
        let mut out: Vec<u8> = Vec::with_capacity(total_len as usize);
        // 12-byte header.
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&total_len.to_le_bytes());
        // Chunk 0: JSON.
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        // Chunk 1: BIN. Type tag is "BIN\0".
        out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0x42, 0x49, 0x4E, 0x00]);
        out.extend_from_slice(&bin_bytes);
        out
    }

    #[test]
    fn parse_glb_extracts_positions_and_indices() {
        let bytes = make_minimal_glb();
        let (vertices, indices) = parse_glb_bytes(&bytes).expect("decode");
        assert_eq!(vertices.len(), 3);
        assert_eq!(indices, vec![0, 1, 2]);
        assert_eq!(vertices[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(vertices[2].position, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn parse_glb_fills_missing_normals_with_up() {
        // The fixture omits NORMAL — loader fills with [0, 1, 0]
        // so the mesh still shades against the directional light.
        let bytes = make_minimal_glb();
        let (vertices, _) = parse_glb_bytes(&bytes).expect("decode");
        assert_eq!(vertices[0].normal, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn parse_glb_rejects_garbage() {
        // Random bytes — gltf::import_slice should refuse the magic.
        assert!(parse_glb_bytes(b"not a glb").is_err());
    }

    #[test]
    fn load_glb_missing_file_errors() {
        // Path that should never exist on a sane test machine.
        let result = load_glb(".twec_no_such_glb_at_test_time.glb");
        assert!(result.is_err());
    }

    /// One-shot fixture generator: writes the minimal triangle
    /// `.glb` to `examples/assets/triangle.glb`. Marked `#[ignore]`
    /// so it only runs when explicitly requested:
    ///
    ///   cargo test --release write_triangle_glb_fixture -- --ignored
    ///
    /// Re-run after changing `make_minimal_glb` to refresh the
    /// committed file. The committed binary is what
    /// `examples/hello_glb.twe` loads at run time.
    #[test]
    #[ignore]
    fn write_triangle_glb_fixture() {
        let path = std::path::Path::new("examples/assets/triangle.glb");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        let bytes = make_minimal_glb();
        std::fs::write(path, &bytes).expect("write fixture");
        // Round-trip check: the file we just wrote must decode.
        let (vertices, indices) = load_glb(path.to_str().unwrap()).expect("decode");
        assert_eq!(vertices.len(), 3);
        assert_eq!(indices.len(), 3);
    }
}
