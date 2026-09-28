//! web3d-M4: `sound.*` in the browser, through WebAudio.
//!
//! The interpreter queues `audio_host::AudioCmd`s; [`WebAudio::play_queued`]
//! runs them each frame. Each sound's bytes come from the mounted game
//! bundle (or a fetch), are decoded once by `decodeAudioData`, and play
//! through a gain node per instance into a gain node per sound (so
//! `sound.set_volume` / `sound.stop` reach every playing instance).
//!
//! Browsers only start audio after a user gesture; [`WebAudio::unlock`]
//! is called from the first key press or click.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use twec::audio_host::AudioCmd;
use wasm_bindgen::JsCast;

/// One sound: decoded buffer (once ready), its gain node, the
/// instances playing now, and plays that arrived before decoding
/// finished.
struct Voice {
    buffer: Option<web_sys::AudioBuffer>,
    gain: web_sys::GainNode,
    playing: Vec<web_sys::AudioBufferSourceNode>,
    pending: Vec<(f32, bool)>,
    failed: bool,
}

#[derive(Default)]
pub struct WebAudio {
    ctx: Option<web_sys::AudioContext>,
    voices: Rc<RefCell<HashMap<String, Voice>>>,
}

impl WebAudio {
    /// Resume the context after a user gesture (autoplay policy).
    pub fn unlock(&mut self) {
        if let Some(ctx) = self.context() {
            let _ = ctx.resume();
        }
    }

    fn context(&mut self) -> Option<web_sys::AudioContext> {
        if self.ctx.is_none() {
            self.ctx = web_sys::AudioContext::new().ok();
        }
        self.ctx.clone()
    }

    pub fn play_queued(&mut self) {
        let cmds = twec::audio_host::drain();
        if cmds.is_empty() {
            return;
        }
        let Some(ctx) = self.context() else {
            return;
        };
        for cmd in cmds {
            match cmd {
                AudioCmd::Play {
                    path,
                    volume,
                    looped,
                } => self.play(&ctx, &path, volume, looped),
                AudioCmd::Stop { path } => {
                    if let Some(v) = self.voices.borrow_mut().get_mut(&path) {
                        for src in v.playing.drain(..) {
                            #[allow(deprecated)]
                            let _ = src.stop();
                        }
                        v.pending.clear();
                    }
                }
                AudioCmd::SetVolume { path, volume } => {
                    if let Some(v) = self.voices.borrow().get(&path) {
                        v.gain.gain().set_value(volume);
                    }
                }
            }
        }
    }

    fn play(&mut self, ctx: &web_sys::AudioContext, path: &str, volume: f32, looped: bool) {
        let mut voices = self.voices.borrow_mut();
        if !voices.contains_key(path) {
            let Ok(gain) = ctx.create_gain() else {
                return;
            };
            let _ = gain.connect_with_audio_node(&ctx.destination());
            voices.insert(
                path.to_string(),
                Voice {
                    buffer: None,
                    gain,
                    playing: Vec::new(),
                    pending: vec![(volume, looped)],
                    failed: false,
                },
            );
            drop(voices);
            self.decode(ctx, path);
            return;
        }
        let voice = voices.get_mut(path).expect("checked");
        if voice.failed {
            return;
        }
        match voice.buffer.clone() {
            Some(buffer) => start(ctx, voice, &buffer, volume, looped),
            None => voice.pending.push((volume, looped)),
        }
    }

    /// Read and decode `path`, then play whatever was requested while
    /// it decoded.
    fn decode(&self, ctx: &web_sys::AudioContext, path: &str) {
        let bytes = match twec::bundle::read_asset_bytes(path) {
            Ok(b) => b,
            Err(e) => {
                web_sys::console::warn_1(&format!("sound: cannot read '{path}': {e}").into());
                if let Some(v) = self.voices.borrow_mut().get_mut(path) {
                    v.failed = true;
                }
                return;
            }
        };
        let array = js_sys::Uint8Array::from(bytes.as_slice()).buffer();
        let Ok(promise) = ctx.decode_audio_data(&array) else {
            return;
        };
        let voices = self.voices.clone();
        let ctx = ctx.clone();
        let path = path.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let decoded = wasm_bindgen_futures::JsFuture::from(promise).await;
            let mut voices = voices.borrow_mut();
            let Some(voice) = voices.get_mut(&path) else {
                return;
            };
            match decoded
                .ok()
                .and_then(|b| b.dyn_into::<web_sys::AudioBuffer>().ok())
            {
                Some(buffer) => {
                    for (volume, looped) in std::mem::take(&mut voice.pending) {
                        start(&ctx, voice, &buffer, volume, looped);
                    }
                    voice.buffer = Some(buffer);
                }
                None => {
                    web_sys::console::warn_1(
                        &format!("sound: '{path}' is not a format the browser can decode").into(),
                    );
                    voice.failed = true;
                    voice.pending.clear();
                }
            }
        });
    }
}

/// Start one instance of `voice` at `volume`.
fn start(
    ctx: &web_sys::AudioContext,
    voice: &mut Voice,
    buffer: &web_sys::AudioBuffer,
    volume: f32,
    looped: bool,
) {
    let (Ok(src), Ok(gain)) = (ctx.create_buffer_source(), ctx.create_gain()) else {
        return;
    };
    src.set_buffer(Some(buffer));
    src.set_loop(looped);
    gain.gain().set_value(volume);
    let _ = src.connect_with_audio_node(&gain);
    let _ = gain.connect_with_audio_node(&voice.gain);
    #[allow(deprecated)]
    let _ = src.start();
    // Keep only instances that may still be playing: finished one-shots
    // are dropped once the list grows.
    if voice.playing.len() > 32 {
        voice.playing.drain(..16);
    }
    voice.playing.push(src);
}
