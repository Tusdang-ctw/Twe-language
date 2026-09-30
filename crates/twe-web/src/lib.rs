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
use std::rc::Rc;

mod audio;

use twec::kernel::render::{parse_glb_bytes, AssetKind, AssetReady, AssetSource, Renderer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Twe key names with their DOM `KeyboardEvent.code`: the same table
/// the native shell reports (web3d-M4).
use twec::host3d::KEY_CODES as KEYS;
use twec::host3d::InputState;

#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("twe: {info}").into());
    }));
    // `Instant` panics on wasm32; give the runtime the page's clock.
    twec::clock::install_host_clock(now_secs);
    // Loaded outside a page (the Node benchmark): nothing to run.
    if web_sys::window().is_none() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = run().await {
            show_error(&e);
        }
    });
}

thread_local! {
    /// The page's `Performance`, looked up once: `window()` +
    /// `performance()` on every clock read were a measurable share of
    /// the frame (web3d-M3 profiling).
    static PERFORMANCE: Option<web_sys::Performance> = match web_sys::window() {
        Some(w) => w.performance(),
        // Node (the benchmark) has a global `performance` too.
        None => js_sys::Reflect::get(&js_sys::global(), &"performance".into())
            .ok()
            .and_then(|p| p.dyn_into().ok()),
    };
}

/// Headless benchmark (web3d-M3's wasm measurement, run under Node by
/// `web/bench.mjs`): run `source`'s top level, then `ticks` update
/// ticks at 60 Hz, and return every tick's duration in milliseconds.
/// No page, no renderer: the script tick alone, as the exit criterion
/// measures it.
#[wasm_bindgen]
pub fn bench_ticks(source: &str, ticks: u32) -> Result<Vec<f64>, String> {
    let tokens = twec::lexer::lex(source).map_err(|e| e.to_string())?;
    let program = twec::parser::parse(&tokens).map_err(|e| e.to_string())?;
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    twec::eval::run_top_level(&mut env, &program).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(ticks as usize);
    for _ in 0..ticks {
        let t = now_secs();
        twec::eval::tick_frame(&mut env, twec::eval::PHYSICS_DT).map_err(|e| e.to_string())?;
        out.push((now_secs() - t) * 1e3);
    }
    Ok(out)
}

/// web3d-M4: the soak harness under Node (`web/soak.mjs`): mount a
/// built game bundle, play `ticks` fixed ticks with `twec::soak`'s
/// scripted player (optionally with the GC collecting at every
/// safepoint), and return its report as JSON. Saves go to an in-memory
/// store, since Node has no localStorage.
#[wasm_bindgen]
pub fn soak(bundle: &[u8], ticks: u32, variant: u32, gc_stress: bool) -> Result<String, String> {
    let reader = twec::bundle::BundleReader::from_bytes(bundle.to_vec())
        .map_err(|e| format!("bundle: {e}"))?;
    twec::bundle::set_active_bundle(reader);
    let source = twec::bundle::read_asset_bytes("main.twe").map_err(|e| format!("main.twe: {e}"))?;
    let source = String::from_utf8(source).map_err(|_| "main.twe is not UTF-8")?;
    twec::save::install_web_storage(memory_get, memory_set);
    twec::heap::gc_set_stress(gc_stress);
    let report = twec::soak::run(&source, u64::from(ticks), u64::from(variant));
    twec::heap::gc_set_stress(false);
    report.map(|r| r.to_json())
}

thread_local! {
    static MEMORY_STORE: RefCell<std::collections::HashMap<String, String>> =
        RefCell::new(std::collections::HashMap::new());
}

fn memory_get(key: &str) -> Option<String> {
    MEMORY_STORE.with(|m| m.borrow().get(key).cloned())
}

fn memory_set(key: &str, value: &str) -> Result<(), String> {
    MEMORY_STORE.with(|m| m.borrow_mut().insert(key.to_string(), value.to_string()));
    Ok(())
}

fn now_secs() -> f64 {
    PERFORMANCE.with(|p| p.as_ref().map(|p| p.now() / 1000.0).unwrap_or(0.0))
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

/// Rolling frame-time totals since the last `frame_stats()` call.
#[derive(Default)]
struct Stats {
    frames: f64,
    ticks: f64,
    tick_ms: f64,
    script_render_ms: f64,
    kernel_ms: f64,
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::new(Stats::default());
}

/// Averages since the previous call, then resets: `[frames, ticks per
/// frame, ms per tick (script update), ms per frame in the script's
/// render, ms per frame in the kernel]`. For measuring from the page
/// or DevTools (`(await import("./twe_web.js")).frame_stats()`).
#[wasm_bindgen]
pub fn frame_stats() -> Vec<f64> {
    STATS.with(|s| {
        let s = std::mem::take(&mut *s.borrow_mut());
        let per = |x: f64, n: f64| if n > 0.0 { x / n } else { 0.0 };
        vec![
            s.frames,
            per(s.ticks, s.frames),
            per(s.tick_ms, s.ticks),
            per(s.script_render_ms, s.frames),
            per(s.kernel_ms, s.frames),
        ]
    })
}

struct Shell {
    env: twec::value::Env,
    renderer: Renderer,
    assets: WebAssets,
    last_frame: f64,
    accumulator: f64,
    /// Input since the last tick, fed by the DOM listeners; each tick
    /// takes one command from it (`host3d::sim_tick`).
    input: Rc<RefCell<InputState>>,
    audio: audio::WebAudio,
    /// Set by the first key press or click: browsers only let audio
    /// start after a user gesture.
    gestured: Rc<std::cell::Cell<bool>>,
    unlocked: bool,
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

    // The game: the bundle `twec build --target web` writes (the script
    // plus every asset, readable synchronously once mounted). The page
    // downloads it with a progress bar and leaves it in
    // `window.tweBundle`; otherwise fetch `game.twebundle`, or a bare
    // `main.twe` for dev pages.
    let bundle = match preloaded_bundle(&window).await {
        Some(bytes) => Ok(bytes),
        None => fetch_bytes("game.twebundle").await,
    };
    let source_bytes = match bundle {
        Ok(bytes) => {
            let reader = twec::bundle::BundleReader::from_bytes(bytes)
                .map_err(|e| format!("game.twebundle: {e}"))?;
            twec::bundle::set_active_bundle(reader);
            twec::bundle::read_asset_bytes("main.twe").map_err(|e| format!("main.twe: {e}"))?
        }
        Err(_) => fetch_bytes("main.twe").await?,
    };
    let source = String::from_utf8(source_bytes).map_err(|_| "main.twe is not UTF-8")?;
    // web3d-M4: this shell plays `sound.*` (see `audio`), keeps
    // `save.*` / `settings.*` in the page's localStorage, and reports
    // focus for `auto_pause_on_blur`.
    twec::audio_host::enable();
    twec::save::install_web_storage(storage_get, storage_set);
    let tokens = twec::lexer::lex(&source).map_err(|e| format!("main.twe:{e}"))?;
    let program = twec::parser::parse(&tokens).map_err(|e| format!("main.twe:{e}"))?;
    let mut env = twec::value::Env::new();
    twec::stdlib::install(&mut env);
    // web3d-M7: this host simulates compiled particles on the GPU.
    env.gpu_particles = true;
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

    let input = Rc::new(RefCell::new(InputState::default()));
    let gestured = Rc::new(std::cell::Cell::new(false));
    listen_keys(&window, input.clone(), gestured.clone())?;
    listen_focus(&window, &document, input.clone())?;
    listen_pointer(&canvas, input.clone(), gestured.clone())?;

    let shell = Rc::new(RefCell::new(Shell {
        env,
        renderer,
        assets: WebAssets::default(),
        last_frame: now_secs(),
        accumulator: 0.0,
        input,
        audio: audio::WebAudio::default(),
        gestured,
        unlocked: false,
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

    shell.input.borrow_mut().set_gamepad(
        poll_gamepad()
            .as_ref()
            .map(|(buttons, axes)| (&buttons[..], *axes)),
    );

    shell.accumulator += dt;
    let tick_start = now_secs();
    let mut steps = 0;
    while shell.accumulator >= twec::eval::PHYSICS_DT && steps < twec::eval::MAX_SUBSTEPS {
        let mut input = shell.input.borrow_mut();
        if let Err(e) = twec::host3d::sim_tick(&mut shell.env, &mut input, twec::eval::PHYSICS_DT) {
            web_sys::console::error_1(&format!("on update: {e}").into());
        }
        shell.accumulator -= twec::eval::PHYSICS_DT;
        steps += 1;
    }
    if steps >= twec::eval::MAX_SUBSTEPS {
        shell.accumulator = 0.0;
    }
    let tick_ms = (now_secs() - tick_start) * 1e3;

    let Shell {
        env,
        renderer,
        assets,
        ..
    } = shell;
    let times = match twec::host3d::render_frame(renderer, env, assets) {
        Ok(t) => t,
        Err(e) => {
            web_sys::console::error_1(&format!("render: {e}").into());
            Default::default()
        }
    };
    flush_output(env);
    if !shell.unlocked && shell.gestured.get() {
        shell.audio.unlock();
        shell.unlocked = true;
    }
    shell.audio.play_queued();
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.frames += 1.0;
        s.ticks += f64::from(steps);
        s.tick_ms += tick_ms;
        s.script_render_ms += times.script_ms;
        s.kernel_ms += times.kernel_ms;
    });
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
    input: Rc<RefCell<InputState>>,
    gestured: Rc<std::cell::Cell<bool>>,
) -> Result<(), String> {
    let name_of = |code: &str| KEYS.iter().find(|(_, c)| *c == code).map(|(n, _)| *n);
    // Keys the page itself would act on (scrolling, focus moves). All
    // others — F5, F12, shortcuts — stay with the browser.
    let captured = |code: &str| {
        matches!(
            code,
            "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Space" | "Tab" | "Backspace"
        )
    };
    let down_input = input.clone();
    let g = gestured.clone();
    let down =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
            g.set(true);
            if let Some(name) = name_of(&e.code()) {
                // A held key's auto-repeat is not a new press.
                down_input.borrow_mut().key_down(name);
                if captured(&e.code()) {
                    e.prevent_default();
                }
            }
        });
    let up = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
        if let Some(name) = name_of(&e.code()) {
            input.borrow_mut().key_up(name);
        }
    });
    window
        .add_event_listener_with_callback("keydown", down.as_ref().unchecked_ref())
        .map_err(|_| "keydown listener")?;
    window
        .add_event_listener_with_callback("keyup", up.as_ref().unchecked_ref())
        .map_err(|_| "keyup listener")?;
    let click = Closure::<dyn FnMut()>::new(move || gestured.set(true));
    window
        .add_event_listener_with_callback("pointerdown", click.as_ref().unchecked_ref())
        .map_err(|_| "pointerdown listener")?;
    // The listeners live for the page's lifetime.
    down.forget();
    up.forget();
    click.forget();
    Ok(())
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn storage_get(key: &str) -> Option<String> {
    local_storage()?.get_item(key).ok()?
}

fn storage_set(key: &str, value: &str) -> Result<(), String> {
    local_storage()
        .ok_or("this page has no localStorage (private mode or disabled)")?
        .set_item(key, value)
        .map_err(|_| "localStorage is full or refused the write".to_string())
}

thread_local! {
    static BLUR: RefCell<twec::host3d::BlurAutoPause> =
        RefCell::new(twec::host3d::BlurAutoPause::new());
}

/// The page counts as focused while its tab is visible and the window
/// has focus. `requestAnimationFrame` stops in hidden tabs anyway, so
/// the transition is fed from the events, not the frame loop.
fn listen_focus(
    window: &web_sys::Window,
    document: &web_sys::Document,
    input: Rc<RefCell<InputState>>,
) -> Result<(), String> {
    let update = Closure::<dyn FnMut()>::new(move || {
        let focused = web_sys::window()
            .and_then(|w| w.document())
            .map(|d| !d.hidden() && d.has_focus().unwrap_or(true))
            .unwrap_or(true);
        if !focused {
            // Key-ups for keys held now go to another window.
            input.borrow_mut().release_all();
        }
        BLUR.with(|b| b.borrow_mut().tick(focused));
    });
    let f = update.as_ref().unchecked_ref();
    document
        .add_event_listener_with_callback("visibilitychange", f)
        .map_err(|_| "visibilitychange listener")?;
    window
        .add_event_listener_with_callback("blur", f)
        .map_err(|_| "blur listener")?;
    window
        .add_event_listener_with_callback("focus", f)
        .map_err(|_| "focus listener")?;
    update.forget();
    Ok(())
}

/// Mouse on the canvas: position in the HUD's 640×480 canvas units
/// (whatever size the page draws the canvas), buttons, raw motion and
/// wheel. The context menu is suppressed so right-click reaches the game.
fn listen_pointer(
    canvas: &web_sys::HtmlCanvasElement,
    input: Rc<RefCell<InputState>>,
    gestured: Rc<std::cell::Cell<bool>>,
) -> Result<(), String> {
    let to_canvas = {
        let canvas = canvas.clone();
        move |e: &web_sys::MouseEvent| {
            let w = f64::from(canvas.client_width().max(1));
            let h = f64::from(canvas.client_height().max(1));
            (
                f64::from(e.offset_x()) * f64::from(twec::kernel::hud::CANVAS_W) / w,
                f64::from(e.offset_y()) * f64::from(twec::kernel::hud::CANVAS_H) / h,
            )
        }
    };
    let button_name = |b: i16| match b {
        0 => Some("left"),
        1 => Some("middle"),
        2 => Some("right"),
        _ => None,
    };

    let (i, pos) = (input.clone(), to_canvas.clone());
    let moved = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
        let (x, y) = pos(&e);
        let mut input = i.borrow_mut();
        input.mouse_move(x, y);
        input.mouse_motion(f64::from(e.movement_x()), f64::from(e.movement_y()));
    });
    let (i, pos) = (input.clone(), to_canvas);
    let down = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
        gestured.set(true);
        let (x, y) = pos(&e);
        let mut input = i.borrow_mut();
        input.mouse_move(x, y);
        if let Some(name) = button_name(e.button()) {
            input.button_down(name);
        }
    });
    let i = input.clone();
    let up = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
        if let Some(name) = button_name(e.button()) {
            i.borrow_mut().button_up(name);
        }
    });
    // DOM wheel: +y scrolls down, in pixels or lines. Twe's `mouse.wheel`
    // is ticks, +y up (as winit and macroquad report it).
    let wheel = Closure::<dyn FnMut(web_sys::WheelEvent)>::new(move |e: web_sys::WheelEvent| {
        e.prevent_default();
        let ticks = match e.delta_mode() {
            web_sys::WheelEvent::DOM_DELTA_PIXEL => e.delta_y() / 100.0,
            _ => e.delta_y(),
        };
        input.borrow_mut().wheel(-ticks);
    });
    let no_menu = Closure::<dyn FnMut(web_sys::Event)>::new(|e: web_sys::Event| {
        e.prevent_default();
    });
    let add = |name: &str, f: &js_sys::Function| {
        canvas
            .add_event_listener_with_callback(name, f)
            .map_err(|_| format!("{name} listener"))
    };
    add("pointermove", moved.as_ref().unchecked_ref())?;
    add("pointerdown", down.as_ref().unchecked_ref())?;
    // On the window: a button released off the canvas still lets go.
    web_sys::window()
        .ok_or("no window")?
        .add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref())
        .map_err(|_| "pointerup listener")?;
    add("contextmenu", no_menu.as_ref().unchecked_ref())?;
    // Non-passive, so preventDefault keeps the page from scrolling.
    let opts = web_sys::AddEventListenerOptions::new();
    opts.set_passive(false);
    canvas
        .add_event_listener_with_callback_and_add_event_listener_options(
            "wheel",
            wheel.as_ref().unchecked_ref(),
            &opts,
        )
        .map_err(|_| "wheel listener")?;
    moved.forget();
    down.forget();
    up.forget();
    wheel.forget();
    no_menu.forget();
    Ok(())
}

/// The first connected gamepad, read through the browser's "standard"
/// mapping into `stdlib::GAMEPAD_BUTTON_NAMES` / `GAMEPAD_AXIS_NAMES`
/// order. Sticks' y is flipped to +y up, as gilrs reports natively.
fn poll_gamepad() -> Option<([bool; 14], [f64; 6])> {
    // Standard-mapping indices for a, b, x, y, lb, rb, lt, rt, start,
    // select, d-pad up / down / left / right.
    const BUTTONS: [u32; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 9, 8, 12, 13, 14, 15];
    let pads = web_sys::window()?.navigator().get_gamepads().ok()?;
    let pad: web_sys::Gamepad = pads
        .iter()
        .filter_map(|p| p.dyn_into::<web_sys::Gamepad>().ok())
        .find(|p| p.connected())?;
    let buttons = pad.buttons();
    let button = |i: u32| buttons.get(i).dyn_into::<web_sys::GamepadButton>().ok();
    let mut held = [false; 14];
    for (slot, i) in held.iter_mut().zip(BUTTONS) {
        *slot = button(i).is_some_and(|b| b.pressed());
    }
    let axes = pad.axes();
    let axis = |i: u32| axes.get(i).as_f64().unwrap_or(0.0);
    let trigger = |i: u32| button(i).map_or(0.0, |b| b.value());
    Some((
        held,
        [axis(0), -axis(1), axis(2), -axis(3), trigger(6), trigger(7)],
    ))
}

/// The bundle the page already downloaded: `window.tweBundle`, a
/// promise of a `Uint8Array` (or `null` if that download failed).
async fn preloaded_bundle(window: &web_sys::Window) -> Option<Vec<u8>> {
    let value = js_sys::Reflect::get(window, &"tweBundle".into()).ok()?;
    let promise: js_sys::Promise = value.dyn_into().ok()?;
    let bytes = wasm_bindgen_futures::JsFuture::from(promise).await.ok()?;
    let bytes: js_sys::Uint8Array = bytes.dyn_into().ok()?;
    Some(bytes.to_vec())
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
        // In the mounted game bundle: ready now, no fetch.
        if twec::bundle::asset_exists(path) {
            let bytes = twec::bundle::read_asset_bytes(path).map_err(|e| e.to_string());
            self.ready.borrow_mut().push(match kind {
                AssetKind::Mesh => AssetReady::Mesh(id, bytes.and_then(|b| parse_glb_bytes(&b))),
                AssetKind::Texture => AssetReady::Texture(id, bytes),
                AssetKind::Environment => AssetReady::Environment(id, bytes),
                AssetKind::Lut => AssetReady::Lut(id, bytes),
            });
            return;
        }
        let ready = self.ready.clone();
        let path = path.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let bytes = fetch_bytes(&path).await;
            let done = match kind {
                AssetKind::Mesh => AssetReady::Mesh(id, bytes.and_then(|b| parse_glb_bytes(&b))),
                AssetKind::Texture => AssetReady::Texture(id, bytes),
                AssetKind::Environment => AssetReady::Environment(id, bytes),
                AssetKind::Lut => AssetReady::Lut(id, bytes),
            };
            ready.borrow_mut().push(done);
        });
    }

    fn poll(&mut self) -> Vec<AssetReady> {
        std::mem::take(&mut *self.ready.borrow_mut())
    }
}
