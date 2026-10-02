//! web3d-M4: native sound for the winit shells (`twec play3d`, and
//! `twec play` since web3d-M6): plays the `audio_host` command queue
//! through `quad-snd`.

use std::collections::{HashMap, HashSet};

/// web3d-M4: the native shells' sound player (3D since M4, 2D since M6). `sound.*` queues
/// commands (`audio_host`) since there is no macroquad window here;
/// this plays them through `quad-snd`, macroquad's own audio backend.
/// The device opens on the first sound, so silent games never touch it.
#[derive(Default)]
pub struct NativeAudio {
    ctx: Option<quad_snd::AudioContext>,
    sounds: HashMap<String, quad_snd::Sound>,
    /// Paths that failed to load: reported once, not every play.
    failed: HashSet<String>,
}

impl NativeAudio {
    pub fn play_queued(&mut self) {
        for cmd in crate::audio_host::drain() {
            match cmd {
                crate::audio_host::AudioCmd::Play {
                    path,
                    volume,
                    looped,
                } => {
                    if self.load(&path) {
                        if let (Some(ctx), Some(sound)) = (&self.ctx, self.sounds.get(&path)) {
                            sound.play(ctx, quad_snd::PlaySoundParams { looped, volume });
                        }
                    }
                }
                crate::audio_host::AudioCmd::Stop { path } => {
                    if let (Some(ctx), Some(sound)) = (&self.ctx, self.sounds.get(&path)) {
                        sound.stop(ctx);
                    }
                }
                crate::audio_host::AudioCmd::SetVolume { path, volume } => {
                    if let (Some(ctx), Some(sound)) = (&self.ctx, self.sounds.get(&path)) {
                        sound.set_volume(ctx, volume);
                    }
                }
            }
        }
    }

    /// Decode `path` on first use (opening the device if needed);
    /// false if it can't be loaded.
    fn load(&mut self, path: &str) -> bool {
        if self.failed.contains(path) {
            return false;
        }
        if !self.sounds.contains_key(path) {
            let ctx = self.ctx.get_or_insert_with(quad_snd::AudioContext::new);
            match crate::bundle::read_asset_bytes(path) {
                Ok(bytes) => {
                    let sound = quad_snd::Sound::load(ctx, &bytes);
                    self.sounds.insert(path.to_string(), sound);
                }
                Err(e) => {
                    eprintln!("sound: cannot read '{path}': {e}");
                    self.failed.insert(path.to_string());
                    return false;
                }
            }
        }
        true
    }
}
