//! Lanternfall E1: the 3D camera as one piece of math, shared by the
//! renderer and the script.
//!
//! The kernel builds its view and projection matrices from
//! [`Camera3d`] with [`perspective`] / [`look_at`]; the `camera.project`
//! / `camera.ray` / `camera.ground_point` builtins use the same
//! functions, so a point the script projects lands on the pixel the
//! GPU draws it at. Pure math with one thread-local (the viewport the
//! host last reported), so it compiles for wasm too.
//!
//! The 3D HUD canvas is 480 units tall and `480 × aspect` wide
//! ([`hud_size`]): at 4:3 that is the 2D canvas's 640×480, and at other
//! aspects text, circles and the minimap keep their proportions.

use std::cell::Cell;

use crate::value::{Env, Value};

/// Camera placement and lens for a frame.
#[derive(Debug, Clone, Copy)]
pub struct Camera3d {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    /// web3d-M7: vertical field of view (radians) and clip planes.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera3d {
    /// The game camera's lens: 60° vertical, 0.1 m to 100 m.
    pub fn new(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Self {
        Camera3d {
            eye,
            target,
            up,
            fov_y: DEFAULT_FOV_DEGREES.to_radians(),
            near: 0.1,
            far: 100.0,
        }
    }
}

/// `camera.fov`'s default, in degrees.
pub const DEFAULT_FOV_DEGREES: f32 = 60.0;

/// The 3D HUD canvas's height in units; its width follows the aspect.
pub const HUD_H: f32 = 480.0;

/// The HUD canvas size for a `width × height` pixel view: 480 units
/// tall, as wide as the aspect makes it. A zero-sized view gets the
/// 2D canvas's 640×480.
pub fn hud_size(width: u32, height: u32) -> [f32; 2] {
    if width == 0 || height == 0 {
        return [640.0, HUD_H];
    }
    let aspect = (width as f32 / height as f32).clamp(0.2, 8.0);
    [HUD_H * aspect, HUD_H]
}

thread_local! {
    /// The view the host last reported (`InputCommand::view_w/h`), in
    /// pixels. 640×480 until a 3D host says otherwise.
    static VIEWPORT: Cell<(u32, u32)> = const { Cell::new((640, 480)) };
    /// Lanternfall E2: this frame's screen shake as a world offset of
    /// the eye and target (set by the host before the frame's
    /// `on render():`; see [`shaken`]).
    static SHAKE: Cell<[f32; 3]> = const { Cell::new([0.0; 3]) };
}

/// Lanternfall E2: a screen shake of `(sx, sy)` HUD units as a world
/// offset of the camera: along its right and up axes, scaled so a unit
/// at the target's distance moves one HUD unit on screen.
pub fn shake_offset(cam: &Camera3d, sx: f32, sy: f32) -> [f32; 3] {
    if sx == 0.0 && sy == 0.0 {
        return [0.0; 3];
    }
    let to = sub(cam.target, cam.eye);
    let dist = dot(to, to).sqrt();
    let f = normalize(to);
    let r = normalize(cross(f, cam.up));
    let u = cross(r, f);
    let per_unit = 2.0 * dist * (cam.fov_y * 0.5).tan() / HUD_H;
    let (dx, dy) = (sx * per_unit, -sy * per_unit);
    [
        r[0] * dx + u[0] * dy,
        r[1] * dx + u[1] * dy,
        r[2] * dx + u[2] * dy,
    ]
}

/// Set this frame's shake offset (the host, once per rendered frame).
pub fn set_shake(offset: [f32; 3]) {
    SHAKE.with(|s| s.set(offset));
}

/// `cam` as this frame draws it: moved by the screen shake.
pub fn shaken(cam: &Camera3d) -> Camera3d {
    let o = SHAKE.with(Cell::get);
    let mut c = *cam;
    for (i, d) in o.iter().enumerate() {
        c.eye[i] += d;
        c.target[i] += d;
    }
    c
}

/// Record the view size the simulation sees. Zeros (a command from a
/// host or replay that didn't report one) leave it as it was.
pub fn set_viewport(width: u32, height: u32) {
    if width > 0 && height > 0 {
        VIEWPORT.with(|v| v.set((width, height)));
    }
}

/// The view size the simulation sees, in pixels.
pub fn viewport() -> (u32, u32) {
    VIEWPORT.with(Cell::get)
}

/// Width over height of the view the simulation sees.
pub fn aspect() -> f32 {
    let (w, h) = viewport();
    w as f32 / h.max(1) as f32
}

/// The script's `camera`: `eye`, `target`, `up`, `far` (web3d-M7
/// session 16) and `fov` (degrees). Missing or malformed fields fall
/// back to the stdlib defaults (eye 3 units back + 1.5 up, looking at
/// the origin, +y up, 60°).
pub fn read_camera(env: &Env) -> Camera3d {
    let eye_default = [0.0, 1.5, 3.0];
    let target_default = [0.0, 0.0, 0.0];
    let up_default = [0.0, 1.0, 0.0];
    let mut camera3d = Camera3d::new(eye_default, target_default, up_default);
    let Some(camera) = env.get("camera").filter(|c| c.is_object()) else {
        return camera3d;
    };
    let rc = camera.as_object();
    let cam = rc.borrow();
    let vec3 = |name: &str| cam.get_field(name).as_ref().and_then(value_as_vec3);
    camera3d.eye = vec3("eye").unwrap_or(eye_default);
    camera3d.target = vec3("target").unwrap_or(target_default);
    camera3d.up = vec3("up").unwrap_or(up_default);
    if let Some(far) = cam.get_field("far").as_ref().and_then(number) {
        // At least a metre past the near plane; finite.
        camera3d.far = (far as f32).clamp(camera3d.near + 1.0, 1.0e6);
    }
    if let Some(fov) = cam.get_field("fov").as_ref().and_then(number) {
        camera3d.fov_y = (fov as f32).clamp(5.0, 170.0).to_radians();
    }
    camera3d
}

pub(crate) fn value_as_vec3(v: &Value) -> Option<[f32; 3]> {
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
        Some(v.as_int() as f64)
    } else if v.is_float() {
        Some(v.as_float())
    } else {
        None
    }
}

/// Where world point `p` appears on a `hud`-sized canvas, or `None`
/// when it is behind the camera. Points off the sides of the view
/// still project (outside `0..hud`), so a script can clamp them to an
/// edge arrow.
pub fn project(cam: &Camera3d, aspect: f32, hud: [f32; 2], p: [f32; 3]) -> Option<[f32; 2]> {
    let view_proj = mul(
        perspective(cam.fov_y, aspect, cam.near, cam.far),
        look_at(cam.eye, cam.target, cam.up),
    );
    let clip = transform(view_proj, [p[0], p[1], p[2], 1.0]);
    if clip[3] <= 1e-6 {
        return None;
    }
    let (nx, ny) = (clip[0] / clip[3], clip[1] / clip[3]);
    Some([(nx * 0.5 + 0.5) * hud[0], (0.5 - ny * 0.5) * hud[1]])
}

/// The ray from the eye through canvas point `xy`: `(origin, unit
/// direction)`. The inverse of [`project`].
pub fn ray(cam: &Camera3d, aspect: f32, hud: [f32; 2], xy: [f32; 2]) -> ([f32; 3], [f32; 3]) {
    let f = normalize(sub(cam.target, cam.eye));
    let r = normalize(cross(f, cam.up));
    let u = cross(r, f);
    let t = (cam.fov_y * 0.5).tan();
    let nx = (xy[0] / hud[0].max(1e-6)) * 2.0 - 1.0;
    let ny = 1.0 - (xy[1] / hud[1].max(1e-6)) * 2.0;
    let (sx, sy) = (nx * t * aspect, ny * t);
    let dir = normalize([
        f[0] + r[0] * sx + u[0] * sy,
        f[1] + r[1] * sx + u[1] * sy,
        f[2] + r[2] * sx + u[2] * sy,
    ]);
    (cam.eye, dir)
}

/// Where the ray through canvas point `xy` meets the horizontal plane
/// `y = plane_y`, or `None` when it runs parallel to it or meets it
/// behind the eye.
pub fn ground_point(
    cam: &Camera3d,
    aspect: f32,
    hud: [f32; 2],
    xy: [f32; 2],
    plane_y: f32,
) -> Option<[f32; 3]> {
    let (o, d) = ray(cam, aspect, hud, xy);
    if d[1].abs() < 1e-6 {
        return None;
    }
    let t = (plane_y - o[1]) / d[1];
    if t < 0.0 {
        return None;
    }
    Some([o[0] + d[0] * t, plane_y, o[2] + d[2] * t])
}

/// Right-handed perspective projection to wgpu's 0..1 depth range
/// (column-major).
pub(crate) fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, (near * far) / (near - far), 0.0],
    ]
}

/// The view matrix looking from `eye` at `target` (column-major).
pub(crate) fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
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

/// `a × b` for column-major 4×4 matrices.
pub(crate) fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
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

fn transform(m: [[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (row, slot) in out.iter_mut().enumerate() {
        *slot = (0..4).map(|k| m[k][row] * v[k]).sum();
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
    let len = dot(v, v).sqrt();
    if len < 1e-12 {
        return v;
    }
    [v[0] / len, v[1] / len, v[2] / len]
}
