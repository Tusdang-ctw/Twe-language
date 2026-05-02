//! `twec play_visual` — wgpu render driver for `visual` blocks.
//!
//! Phase 9 session 11 (the Phase 9 exit gate): take a `.twe` file
//! containing a `visual <Name>:` block, validate the body via
//! `crate::visual_check`, compile to WGSL via `crate::visual_wgsl`,
//! hand it to wgpu, and render the fragment shader fullscreen with a
//! `time: f32` uniform driven from the system clock.
//!
//! Stripped-down compared to `play3d`:
//! - No vertex buffer — the emitted vs_main builds the fullscreen
//!   quad from `vertex_index` (3 verts, no buffer binding).
//! - No depth / no instance buffer — the visual is a single full-
//!   viewport draw call per frame.
//! - No input handling beyond Esc-to-quit (visuals are time-driven,
//!   not interactive in v0.3 — entity attachment is a follow-on).
//! - Single binding group: just the time uniform (binding 0/0).
//!
//! Hot reload: mtime poll on the source file each frame; on change,
//! re-run lex / parse / visual_check / visual_wgsl, rebuild the
//! pipeline. A failed reload keeps the current pipeline alive so a
//! transient typo doesn't tear down the window.

use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use bytemuck::{Pod, Zeroable};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

/// `twec play_visual <file>` entry. Loads the file, finds the first
/// visual block, compiles to WGSL, opens a wgpu window, and renders
/// until the user closes it. Returns the process exit code.
pub fn launch(path: String) -> i32 {
    let wgsl = match build_visual(&path) {
        Ok(w) => w,
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
    let mut app = App {
        path,
        wgsl,
        last_mtime,
        state: None,
        start: Instant::now(),
        exit_code: 0,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("error: event loop: {e}");
        return 1;
    }
    app.exit_code
}

/// Lex / parse / visual_check / compile-to-WGSL the file. Returns
/// the WGSL source for the *first* visual block, or `Err(())` after
/// printing diagnostics if anything fails. Multi-visual files just
/// pick the first; multi-visual rendering rides a follow-on session.
fn build_visual(path: &str) -> Result<String, ()> {
    let src = match std::fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{path}': {e}");
            return Err(());
        }
    };
    let tokens = match crate::lexer::lex(&src) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{path}:{e}");
            return Err(());
        }
    };
    let program = match crate::parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}:{e}");
            return Err(());
        }
    };
    let check_errors = crate::visual_check::check_program(&program);
    if !check_errors.is_empty() {
        for err in &check_errors {
            eprintln!(
                "{path}:{}:{}: {}",
                err.line, err.col, err.message
            );
            if let Some(help) = &err.help {
                eprintln!("  help: {help}");
            }
        }
        return Err(());
    }
    let modules = match crate::visual_wgsl::compile_program(&program) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{path}:{}:{}: visual codegen: {}", e.line, e.col, e.message);
            return Err(());
        }
    };
    let (name, wgsl) = modules.into_iter().next().ok_or_else(|| {
        eprintln!("{path}: no `visual` block found — `twec play_visual` needs at least one");
    })?;
    eprintln!("[twec] play_visual: rendering `visual {name}`");
    Ok(wgsl)
}

fn current_mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(Path::new(path))
        .ok()
        .and_then(|m| m.modified().ok())
}

/// CPU-side mirror of the WGSL `Uniforms` struct. The pad fields
/// satisfy WGSL's vec4-alignment rule for uniform buffers — without
/// them naga would reject the binding.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct VisualUniforms {
    time: f32,
    _pad: [f32; 3],
}

struct RenderState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
}

struct App {
    path: String,
    wgsl: String,
    last_mtime: Option<SystemTime>,
    state: Option<RenderState>,
    start: Instant,
    exit_code: i32,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("Twec play_visual")
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
        match init_wgpu(window.clone(), &self.wgsl) {
            Ok(state) => self.state = Some(state),
            Err(e) => {
                eprintln!("error: wgpu init: {e}");
                self.exit_code = 1;
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
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
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(KeyCode::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                // Hot reload: re-build the WGSL + rebuild the
                // pipeline on file change. Failed reloads keep the
                // existing pipeline alive — same convention as
                // `play3d::RedrawRequested`.
                let cur_mtime = current_mtime(&self.path);
                if cur_mtime.is_some() && cur_mtime != self.last_mtime {
                    if let Ok(new_wgsl) = build_visual(&self.path) {
                        match rebuild_pipeline(state, &new_wgsl) {
                            Ok(()) => {
                                eprintln!("[twec] hot reload: {}", self.path);
                                self.wgsl = new_wgsl;
                            }
                            Err(e) => {
                                eprintln!("[twec] hot reload failed: {e}");
                            }
                        }
                    }
                    self.last_mtime = cur_mtime;
                }

                let elapsed = self.start.elapsed().as_secs_f32();
                if let Err(e) = render(state, elapsed) {
                    eprintln!("render error: {e}");
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn init_wgpu(window: Arc<Window>, wgsl: &str) -> Result<RenderState, String> {
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
            label: Some("twec-play_visual device"),
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

    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("twec-play_visual uniforms"),
        size: std::mem::size_of::<VisualUniforms>() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("twec-play_visual bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            // Visible to the fragment stage; that's where `u.time` is
            // sampled inside `twe_pixel`.
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("twec-play_visual bg"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });

    let pipeline = build_pipeline(&device, &bind_group_layout, surface_format, wgsl)?;

    Ok(RenderState {
        window,
        surface,
        device,
        queue,
        config,
        pipeline,
        bind_group,
        uniform_buffer,
        bind_group_layout,
    })
}

/// Build (or rebuild on hot-reload) the render pipeline from a WGSL
/// string. Surfaced as its own fn so the hot-reload path can call
/// it without re-doing surface / device / uniform-buffer work.
fn build_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    surface_format: wgpu::TextureFormat,
    wgsl: &str,
) -> Result<wgpu::RenderPipeline, String> {
    // wgpu's create_shader_module panics on a parse error rather than
    // returning Result. The visual_wgsl module's snapshot test +
    // naga validation test (tests/visual_wgsl.rs) catch this in CI;
    // at runtime we trust those validations.
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("twec-play_visual shader"),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("twec-play_visual pipeline layout"),
        bind_group_layouts: &[bind_group_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("twec-play_visual pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            // No vertex buffers — vs_main builds the fullscreen quad
            // from vertex_index alone.
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            // Front-face / cull-mode: fullscreen-triangle covers the
            // viewport regardless of orientation; disable culling so
            // the orientation of the trick-triangle doesn't matter.
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn rebuild_pipeline(state: &mut RenderState, wgsl: &str) -> Result<(), String> {
    let pipeline = build_pipeline(
        &state.device,
        &state.bind_group_layout,
        state.config.format,
        wgsl,
    )?;
    state.pipeline = pipeline;
    Ok(())
}

fn render(state: &RenderState, time: f32) -> Result<(), String> {
    let frame = state
        .surface
        .get_current_texture()
        .map_err(|e| e.to_string())?;
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("twec-play_visual encoder"),
        });
    let uniforms = VisualUniforms {
        time,
        _pad: [0.0; 3],
    };
    state
        .queue
        .write_buffer(&state.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("twec-play_visual pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rpass.set_pipeline(&state.pipeline);
        rpass.set_bind_group(0, &state.bind_group, &[]);
        // Three verts, one instance — vs_main reads vertex_index
        // and emits the fullscreen-triangle quad.
        rpass.draw(0..3, 0..1);
    }
    state.queue.submit(Some(encoder.finish()));
    frame.present();
    Ok(())
}
