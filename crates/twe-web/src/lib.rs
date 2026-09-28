//! web3d-M2: Twe's browser shell.
//!
//! Loads `main.twe` next to the page, runs it in the Twe interpreter,
//! and draws every frame through the same renderer kernel as the native
//! `twec play3d`, on the browser's WebGPU. The frame logic is shared
//! with native via `twec::host3d`; this crate only supplies what a
//! browser does differently: the canvas surface, `requestAnimationFrame`,
//! `fetch` for assets, DOM keyboard events and `performance.now()`.
//!
//! Built for `wasm32-unknown-unknown`; `twec build --target web` ships
//! the prebuilt runtime with a game's scripts and assets. On any other
//! target the crate is empty, so `cargo test --workspace` stays native.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use twec::kernel::render::{parse_glb_bytes, AssetKind, AssetReady, AssetSource, Renderer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Twe key names the browser shell reports, with their DOM
/// `KeyboardEvent.code`. (Full key coverage follows; these are what the
/// 3D examples read.)
const KEYS: &[(&str, &str)] = &[
    ("left", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("up", "ArrowUp"),
    ("down", "ArrowDown"),
    ("a", "KeyA"),
    ("d", "KeyD"),
    ("w", "KeyW"),
    ("s", "KeyS"),
    ("space", "Space"),
    ("enter", "Enter"),
    ("escape", "Escape"),
];

#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("twe: {info}").into());
    }));
    // `Instant` panics on wasm32; give the runtime the page's clock.
    twec::clock::install_host_clock(now_secs);
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = run().await {
            show_error(&e);
        }
    });
}

fn now_secs() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now() / 1000.0)
        .unwrap_or(0.0)
}

/// Log to the console and, if the page has one, into `#twe-status`.
fn show_error(msg: &str) {
    web_sys::console::error_1(&msg.into());
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("twe-status"))
    {
        el.set_text_content(Some(msg));
    }
}

struct Shell {
    env: twec::value::Env,
    renderer: Renderer,
    assets: WebAssets,
    last_frame: f64,
    accumulator: f64,
    held: Rc<RefCell<HashSet<String>>>,
    pressed: Rc<RefCell<HashSet<String>>>,
}

async fn run() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let canvas: web_sys::HtmlCanvasElement = document
        .get_element_by_id("twe-canvas")
        .ok_or("page has no <canvas id=\"twe-canvas\">")?
        .dyn_into()
        .map_err(|_| "#twe-canvas is not a canvas")?;
    let (width, height) = (canvas.width().max(1), canvas.height().max(1));

    // The game: `main.twe` next to the page.
    let source = String::from_utf8(fetch_bytes("main.twe").await?)
        .map_err(|_| "main.twe is not UTF-8")?;
    let tokens = twec::lexer::lex(&source).map_err(|e| format!("main.twe:{e}"))?;
    let program = twec::parser::parse(&tokens).map_err(|e| format!("main.twe:{e}"))?;
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program)
        .map_err(|e| format!("main.twe: runtime error: {e}"))?;
    flush_output(&mut env);

    // WebGPU on the canvas.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| format!("WebGPU surface: {e}"))?;
    let renderer = Renderer::new(&instance, surface, width, height)
        .await
        .map_err(|e| format!("WebGPU is required to run this game ({e})"))?;

    let held = Rc::new(RefCell::new(HashSet::new()));
    let pressed = Rc::new(RefCell::new(HashSet::new()));
    listen_keys(&window, held.clone(), pressed.clone())?;

    let shell = Rc::new(RefCell::new(Shell {
        env,
        renderer,
        assets: WebAssets::default(),
        last_frame: now_secs(),
        accumulator: 0.0,
        held,
        pressed,
    }));
    start_frame_loop(shell);
    if let Some(el) = document.get_element_by_id("twe-status") {
        el.set_text_content(None);
    }
    Ok(())
}

/// One animation frame: fixed-step simulation, then draw.
fn frame(shell: &mut Shell) {
    let now = now_secs();
    let dt = (now - shell.last_frame).min(twec::eval::MAX_FRAME_DT);
    shell.last_frame = now;

    {
        let held = shell.held.borrow();
        let pressed = shell.pressed.borrow();
        let names: Vec<&str> = KEYS.iter().map(|(n, _)| *n).collect();
        twec::host3d::apply_key_state(
            &mut shell.env,
            &names,
            &|n| held.contains(n),
            &|n| pressed.contains(n),
        );
    }
    shell.pressed.borrow_mut().clear();

    shell.accumulator += dt;
    let mut steps = 0;
    while shell.accumulator >= twec::eval::PHYSICS_DT && steps < twec::eval::MAX_SUBSTEPS {
        if let Err(e) = twec::eval::tick_frame(&mut shell.env, twec::eval::PHYSICS_DT) {
            web_sys::console::error_1(&format!("on update: {e}").into());
        }
        shell.accumulator -= twec::eval::PHYSICS_DT;
        steps += 1;
    }
    if steps >= twec::eval::MAX_SUBSTEPS {
        shell.accumulator = 0.0;
    }

    let Shell {
        env,
        renderer,
        assets,
        ..
    } = shell;
    if let Err(e) = twec::host3d::render_frame(renderer, env, assets) {
        web_sys::console::error_1(&format!("render: {e}").into());
    }
    flush_output(env);
}

fn flush_output(env: &mut twec::value::Env) {
    if !env.out.is_empty() {
        for line in env.out.lines() {
            web_sys::console::log_1(&line.into());
        }
        env.out.clear();
    }
}

/// Drive `frame` from `requestAnimationFrame`.
fn start_frame_loop(shell: Rc<RefCell<Shell>>) {
    type Tick = Closure<dyn FnMut()>;
    let slot: Rc<RefCell<Option<Tick>>> = Rc::new(RefCell::new(None));
    let next = slot.clone();
    *slot.borrow_mut() = Some(Closure::new(move || {
        frame(&mut shell.borrow_mut());
        request_frame(next.borrow().as_ref().expect("frame closure"));
    }));
    request_frame(slot.borrow().as_ref().expect("frame closure"));
}

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
    }
}

fn listen_keys(
    window: &web_sys::Window,
    held: Rc<RefCell<HashSet<String>>>,
    pressed: Rc<RefCell<HashSet<String>>>,
) -> Result<(), String> {
    let name_of = |code: &str| KEYS.iter().find(|(_, c)| *c == code).map(|(n, _)| *n);
    let (h, p) = (held.clone(), pressed);
    let down = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
        if let Some(name) = name_of(&e.code()) {
            if h.borrow_mut().insert(name.to_string()) {
                p.borrow_mut().insert(name.to_string());
            }
            e.prevent_default();
        }
    });
    let up = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
        if let Some(name) = name_of(&e.code()) {
            held.borrow_mut().remove(name);
        }
    });
    window
        .add_event_listener_with_callback("keydown", down.as_ref().unchecked_ref())
        .map_err(|_| "keydown listener")?;
    window
        .add_event_listener_with_callback("keyup", up.as_ref().unchecked_ref())
        .map_err(|_| "keyup listener")?;
    // The listeners live for the page's lifetime.
    down.forget();
    up.forget();
    Ok(())
}

/// `fetch` a URL relative to the page and return its body.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|_| format!("fetch `{url}` failed"))?
        .dyn_into()
        .map_err(|_| "fetch did not return a Response")?;
    if !resp.ok() {
        return Err(format!("fetch `{url}`: HTTP {}", resp.status()));
    }
    let buf = wasm_bindgen_futures::JsFuture::from(
        resp.array_buffer().map_err(|_| format!("read `{url}`"))?,
    )
    .await
    .map_err(|_| format!("read `{url}`"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Browser asset loading for the kernel: `fetch` each requested file;
/// meshes are parsed as they arrive (the page is single-threaded).
#[derive(Default)]
struct WebAssets {
    ready: Rc<RefCell<Vec<AssetReady>>>,
}

impl AssetSource for WebAssets {
    fn request(&mut self, kind: AssetKind, id: u32, path: &str) {
        let ready = self.ready.clone();
        let path = path.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let bytes = fetch_bytes(&path).await;
            let done = match kind {
                AssetKind::Mesh => AssetReady::Mesh(id, bytes.and_then(|b| parse_glb_bytes(&b))),
                AssetKind::Texture => AssetReady::Texture(id, bytes),
            };
            ready.borrow_mut().push(done);
        });
    }

    fn poll(&mut self) -> Vec<AssetReady> {
        std::mem::take(&mut *self.ready.borrow_mut())
    }
}
