//! web3d-M6: `twec play` — the native 2D shell.
//!
//! A winit window drawn by the kernel's 2D canvas (`kernel::canvas`),
//! replacing the macroquad loops in `play.rs`. A frame:
//!
//! 1. hot reload (debounced, `play::ReloadGate`), gamepad poll, touch
//!    taps, cursor mode, idle / blur auto-pause;
//! 2. `frame2d::tick` — fixed-step simulation, one input command per
//!    tick from the shell's [`InputState`] (so replays and the input
//!    stream work as in 3D);
//! 3. `frame2d::render` — the script's render code into a draw list;
//! 4. the canvas draws the list (and the F3 frame-time panel) into the
//!    window, the 640×480 canvas scaled to fit and letterboxed;
//! 5. queued sounds play, F12 / `screenshot()` / `TWE_CAPTURE` write
//!    PNGs at the canvas's own 640×480.
//!
//! Colour blending happens in gamma space (a non-sRGB view of the
//! surface), as macroquad blended, so pictures match the references in
//! `tests/references/m6/`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::draw2d::Draw2d;
use crate::host3d::{BlurAutoPause, InputState};
use crate::kernel::canvas::{Canvas, Layer, SyncAssets, CANVAS_H, CANVAS_W};
use crate::play::ReloadGate;
use crate::value::Env;

/// `twec play <file>`: load the script, open the window, run until it
/// closes. Returns the exit code.
pub fn launch(path: String) -> i32 {
    let env = match crate::frame2d::load(&path) {
        Ok(env) => env,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    let gate = ReloadGate::new(mtime(&path));
    run(env, Some((path, gate)))
}

/// Phase 12 session 4: a shipped build's `main.twe`, from its bundle
/// (no hot reload: the source lives in the `.exe`).
pub fn launch_embedded(source: String) -> i32 {
    match crate::frame2d::load_source(&source, "<embedded>main.twe") {
        Ok(env) => run(env, None),
        Err(msg) => {
            eprintln!("{msg}");
            1
        }
    }
}

fn run(mut env: Env, file: Option<(String, ReloadGate)>) -> i32 {
    // This shell plays `sound.*` itself (`native_audio`).
    crate::audio_host::enable();
    // Phase 15 session 3 / web3d-M0: Steam, in dev and shipped builds.
    crate::steam::init();
    flush(&mut env);
    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("error: could not create event loop: {e}");
            return 1;
        }
    };
    let mut app = App::new(env, file);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("error: event loop: {e}");
        return 1;
    }
    app.exit_code
}

fn mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn flush(env: &mut Env) {
    if !env.out.is_empty() {
        print!("{}", env.out);
        env.out.clear();
    }
}

/// The window, its surface and the canvas drawing into it.
struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// A non-sRGB view of the surface: blending in gamma space.
    view_format: wgpu::TextureFormat,
    canvas: Canvas,
}

impl Gpu {
    fn new(window: Arc<Window>) -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: crate::kernel::render::power_preference(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no compatible wgpu adapter found: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("twe-play2d device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| e.to_string())?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| !f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);
        let view_format = format.remove_srgb_suffix();
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: if view_format == format {
                vec![]
            } else {
                vec![view_format]
            },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let canvas = Canvas::new(&device, &queue, view_format);
        Ok(Gpu {
            window,
            surface,
            device,
            queue,
            config,
            view_format,
            canvas,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// The canvas's rectangle in the window: the largest 4:3 fit,
    /// centred (letterboxed).
    fn viewport(&self) -> [f32; 4] {
        let (w, h) = (self.config.width as f32, self.config.height as f32);
        let scale = (w / CANVAS_W).min(h / CANVAS_H);
        let (vw, vh) = (
            (CANVAS_W * scale).floor().max(1.0),
            (CANVAS_H * scale).floor().max(1.0),
        );
        [((w - vw) / 2.0).floor(), ((h - vh) / 2.0).floor(), vw, vh]
    }

    /// Draw `layers` into the window.
    fn present(&mut self, layers: &[Layer<'_>], assets: &mut SyncAssets) -> Result<(), String> {
        let [x, y, w, h] = self.viewport();
        self.canvas.prepare_layers_loaded(
            &self.device,
            &self.queue,
            layers,
            w as u32,
            h as u32,
            assets,
        );
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(())
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface configuration failed validation".to_string());
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        self.draw_into(&view, Some([x, y, w, h]));
        self.queue.present(frame);
        Ok(())
    }

    fn draw_into(&self, view: &wgpu::TextureView, viewport: Option<[f32; 4]>) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("twe-play2d pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some([x, y, w, h]) = viewport {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
            }
            self.canvas.draw(&mut pass);
        }
        self.queue.submit(Some(encoder.finish()));
    }

    /// Draw `layers` at the canvas's own 640×480 and return the image
    /// (screenshots and `TWE_CAPTURE`, whatever the window size).
    fn snapshot(
        &mut self,
        layers: &[Layer<'_>],
        assets: &mut SyncAssets,
    ) -> Result<image::RgbaImage, String> {
        let (w, h) = (CANVAS_W as u32, CANVAS_H as u32);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("twe-play2d snapshot"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.view_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.canvas
            .prepare_layers_loaded(&self.device, &self.queue, layers, w, h, assets);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.draw_into(&view, None);
        let mut pixels =
            crate::kernel::canvas::read_texture(&self.device, &self.queue, &texture, w, h)?;
        if matches!(self.view_format, wgpu::TextureFormat::Bgra8Unorm) {
            for px in pixels.chunks_mut(4) {
                px.swap(0, 2);
            }
        }
        image::RgbaImage::from_raw(w, h, pixels).ok_or_else(|| "snapshot size".to_string())
    }
}

/// Phase 11 session 11: pause after `auto_pause_when_idle(seconds)` with
/// no input, and resume on input if this drove the pause.
#[derive(Default)]
struct IdlePause {
    idle_secs: f64,
    paused_by_us: bool,
}

impl IdlePause {
    fn tick(&mut self, dt: f64, active: bool) {
        self.idle_secs = if active { 0.0 } else { self.idle_secs + dt };
        let threshold = crate::stdlib::auto_pause_idle_threshold();
        if threshold <= 0.0 {
            self.paused_by_us = false;
        } else if self.idle_secs >= threshold {
            if !crate::stdlib::is_paused() {
                crate::stdlib::set_paused(true);
                self.paused_by_us = true;
            }
        } else if self.paused_by_us && self.idle_secs == 0.0 {
            crate::stdlib::set_paused(false);
            self.paused_by_us = false;
        }
    }
}

/// Phase 11 session 2: the F3 frame-time panel's recent frames.
const HUD_HISTORY: usize = 120;

struct App {
    env: Env,
    /// The script file and its reload gate; `None` for a shipped build.
    file: Option<(String, ReloadGate)>,
    gpu: Option<Gpu>,
    assets: SyncAssets,
    input: InputState,
    accumulator: f64,
    audio: crate::native_audio::NativeAudio,
    gilrs: Option<gilrs::Gilrs>,
    blur: BlurAutoPause,
    idle: IdlePause,
    focused: bool,
    /// Any key, button, wheel or cursor event since the last frame.
    active: bool,
    /// Text typed since the last frame, for `text_input`.
    typed: String,
    hud_visible: bool,
    hud_frames: VecDeque<f64>,
    screenshot_key: bool,
    /// `TWE_CAPTURE=N:path`: write frame N and exit.
    capture: Option<(u32, String)>,
    frame_no: u32,
    started_at: Instant,
    last_frame_at: Instant,
    exit_code: i32,
}

impl App {
    fn new(env: Env, file: Option<(String, ReloadGate)>) -> App {
        App {
            env,
            file,
            gpu: None,
            assets: SyncAssets::default(),
            input: InputState::default(),
            accumulator: 0.0,
            audio: crate::native_audio::NativeAudio::default(),
            gilrs: gilrs::Gilrs::new()
                .map_err(|e| eprintln!("[twec] gamepad disabled: {e}"))
                .ok(),
            blur: BlurAutoPause::new(),
            idle: IdlePause::default(),
            focused: true,
            active: false,
            typed: String::new(),
            hud_visible: false,
            hud_frames: VecDeque::with_capacity(HUD_HISTORY),
            screenshot_key: false,
            capture: capture_request(),
            frame_no: 0,
            started_at: Instant::now(),
            last_frame_at: Instant::now(),
            exit_code: 0,
        }
    }

    /// Phase 11 session 4: debounced hot reload of the script file. A
    /// failed reload keeps the running game.
    fn hot_reload(&mut self) {
        let Some((path, gate)) = self.file.as_mut() else {
            return;
        };
        if !gate.should_reload(mtime(path)) {
            return;
        }
        match crate::frame2d::load(path) {
            Ok(env) => {
                eprintln!("[twec] hot reload: {path}");
                crate::stdlib::clear_asset_caches();
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.canvas.clear_asset_caches();
                }
                self.assets = SyncAssets::default();
                self.env = env;
                self.accumulator = 0.0;
                flush(&mut self.env);
            }
            Err(msg) => {
                eprintln!("{msg}");
                eprintln!("[twec] hot reload failed; keeping previous script live");
            }
        }
    }

    /// The F3 panel: current / average / worst frame time, top right.
    fn hud(&self) -> Vec<Draw2d> {
        if !self.hud_visible || self.hud_frames.is_empty() {
            return Vec::new();
        }
        let n = self.hud_frames.len() as f64;
        let cur = self.hud_frames.back().copied().unwrap_or(0.0) * 1000.0;
        let avg = self.hud_frames.iter().sum::<f64>() / n * 1000.0;
        let max = self.hud_frames.iter().copied().fold(0.0, f64::max) * 1000.0;
        let fps = if avg > 0.0 { 1000.0 / avg } else { 0.0 };
        let text =
            format!("frame {cur:>5.1}ms  avg {avg:>5.1}ms  max {max:>5.1}ms  {fps:>5.1} fps");
        let dim = crate::draw2d::measure_text(&text, 14);
        let pad = 6.0;
        let x = CANVAS_W - dim.width - pad * 2.0;
        let mut out = Vec::new();
        crate::draw2d::rect(
            &mut out,
            x,
            pad,
            dim.width + pad * 2.0,
            dim.height + pad * 2.0,
            [0.0, 0.0, 0.0, 0.7],
        );
        crate::draw2d::text(
            &mut out,
            &text,
            x + pad,
            pad + pad + dim.height,
            14.0,
            [1.0; 4],
        );
        out
    }

    /// One frame: simulate, render, draw, save images, play sounds.
    /// Returns false when the game should end.
    fn frame(&mut self) -> bool {
        self.hot_reload();
        crate::stdlib::tick_touch_taps(self.started_at.elapsed().as_secs_f64());
        if let Some(g) = self.gilrs.as_mut().filter(|_| self.capture.is_none()) {
            let pad = crate::play::read_first_pad(g);
            if pad.is_some() {
                self.active = true;
            }
            self.input
                .set_gamepad(pad.as_ref().map(|(b, a)| (&b[..], *a)));
        }
        if let (Some(mode), Some(gpu)) =
            (crate::stdlib::take_pending_cursor_mode(), self.gpu.as_ref())
        {
            crate::play3d::apply_cursor_mode(&gpu.window, mode);
        }
        let now = Instant::now();
        // A capture advances exactly one tick per frame, so frame N is
        // the same picture on every machine.
        let frame_dt = if self.capture.is_some() {
            crate::eval::PHYSICS_DT
        } else {
            now.duration_since(self.last_frame_at)
                .as_secs_f64()
                .min(crate::eval::MAX_FRAME_DT)
        };
        self.last_frame_at = now;
        if self.hud_frames.len() == HUD_HISTORY {
            self.hud_frames.pop_front();
        }
        self.hud_frames.push_back(frame_dt);
        self.idle.tick(frame_dt, std::mem::take(&mut self.active));
        self.blur.tick(self.focused);

        if let Err(e) = crate::frame2d::tick(
            &mut self.env,
            Some(&mut self.input),
            &mut self.accumulator,
            frame_dt,
        ) {
            eprintln!("{}: runtime error: {e}", self.label());
            self.exit_code = 1;
            return false;
        }
        flush(&mut self.env);
        crate::audio_polish::tick(frame_dt);
        crate::stdlib::sync_audio_polish();
        crate::stdlib::set_typed_text(std::mem::take(&mut self.typed));
        let (camera, result) = crate::frame2d::render(&mut self.env, frame_dt, CANVAS_W, CANVAS_H);
        let hud = self.hud();
        let items = std::mem::take(&mut self.env.draw2d);
        let layers: [Layer<'_>; 2] = [(&items, camera), (&hud, None)];
        if let Some(gpu) = self.gpu.as_mut() {
            if let Err(e) = gpu.present(&layers, &mut self.assets) {
                eprintln!("error: {e}");
            }
            // Phase 11 session 1: F12, or a queued `screenshot(path)`.
            let shot = crate::stdlib::take_pending_screenshot()
                .or_else(|| std::mem::take(&mut self.screenshot_key).then(default_screenshot_path));
            self.frame_no += 1;
            let capture = self
                .capture
                .as_ref()
                .filter(|(at, _)| *at == self.frame_no)
                .map(|(_, path)| path.clone());
            for path in shot.iter().chain(capture.iter()) {
                match gpu
                    .snapshot(&layers, &mut self.assets)
                    .and_then(|img| img.save(path).map_err(|e| e.to_string()))
                {
                    Ok(()) => eprintln!("[twec] screenshot saved: {path}"),
                    Err(e) => eprintln!("[twec] screenshot failed: {path}: {e}"),
                }
            }
            if capture.is_some() {
                return false;
            }
        }
        self.env.draw2d = items;
        self.audio.play_queued();
        if let Err(e) = result {
            eprintln!("{}: runtime error: {e}", self.label());
            self.exit_code = 1;
            return false;
        }
        flush(&mut self.env);
        !crate::stdlib::take_quit_request()
    }

    fn label(&self) -> &str {
        self.file
            .as_ref()
            .map_or("<embedded>main.twe", |(p, _)| p.as_str())
    }

    /// Window pixels → canvas coordinates, through the letterbox.
    fn to_canvas(&self, x: f64, y: f64) -> (f64, f64) {
        match self.gpu.as_ref() {
            Some(gpu) => {
                let [vx, vy, vw, vh] = gpu.viewport();
                (
                    (x - f64::from(vx)) * f64::from(CANVAS_W) / f64::from(vw),
                    (y - f64::from(vy)) * f64::from(CANVAS_H) / f64::from(vh),
                )
            }
            None => (x, y),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("Twec play")
            .with_inner_size(LogicalSize::new(f64::from(CANVAS_W), f64::from(CANVAS_H)));
        let created = event_loop
            .create_window(attrs)
            .map_err(|e| e.to_string())
            .and_then(|w| Gpu::new(Arc::new(w)));
        match created {
            Ok(gpu) => {
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
            }
            Err(e) => {
                eprintln!("error: window / wgpu init: {e}");
                self.exit_code = 1;
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // A capture runs with no input, so frame N is the same picture
        // wherever the cursor happens to be.
        let input_event = matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
        );
        if input_event && self.capture.is_some() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(focused) => {
                if !focused {
                    // Key-ups won't arrive while unfocused.
                    self.input.release_all();
                }
                self.focused = focused;
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        text,
                        ..
                    },
                ..
            } => {
                self.active = true;
                if let Some(name) = crate::play3d::winit_key_name(code) {
                    match state {
                        ElementState::Pressed if !repeat => self.input.key_down(name),
                        ElementState::Pressed => {}
                        ElementState::Released => self.input.key_up(name),
                    }
                }
                if state == ElementState::Pressed {
                    if let Some(text) = text {
                        self.typed.push_str(&text);
                    }
                    if !repeat {
                        match code {
                            KeyCode::F3 => self.hud_visible = !self.hud_visible,
                            KeyCode::F12 => self.screenshot_key = true,
                            // web3d-M0: Escape quits unless the script
                            // took it over with `quit_on_escape(false)`.
                            KeyCode::Escape if crate::stdlib::quit_on_escape() => event_loop.exit(),
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.active = true;
                let (x, y) = self.to_canvas(position.x, position.y);
                self.input.mouse_move(x, y);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.active = true;
                if let Some(name) = crate::play3d::mouse_button_name(button) {
                    match state {
                        ElementState::Pressed => self.input.button_down(name),
                        ElementState::Released => self.input.button_up(name),
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.active = true;
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 120.0,
                };
                self.input.wheel(dy);
            }
            WindowEvent::RedrawRequested => {
                if !self.frame() {
                    event_loop.exit();
                    return;
                }
                if let Some(gpu) = self.gpu.as_ref() {
                    gpu.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.capture.is_some() {
            return;
        }
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.input.mouse_motion(dx, dy);
        }
    }
}

/// `TWE_CAPTURE=N:path` (see `play.rs`): write frame `N`, then exit.
fn capture_request() -> Option<(u32, String)> {
    let spec = std::env::var("TWE_CAPTURE").ok()?;
    let (n, path) = spec.split_once(':')?;
    match n.trim().parse::<u32>() {
        Ok(n) if n > 0 && !path.is_empty() => Some((n, path.to_string())),
        _ => {
            eprintln!(
                "[twec] TWE_CAPTURE should be FRAME:PATH, e.g. 60:out.png; ignoring '{spec}'"
            );
            None
        }
    }
}

fn default_screenshot_path() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("screenshot-{secs}.png")
}
