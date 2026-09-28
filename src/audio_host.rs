//! web3d-M4: sound outside the 2D runtime.
//!
//! In 2D, `sound.*` plays through macroquad's audio. The 3D shells
//! (native `twec play3d`, and the browser) have no macroquad context,
//! so there the builtins queue [`AudioCmd`]s and the shell plays them:
//! natively through `quad-snd`, in the browser through WebAudio. The
//! pool throttle, ducking and scheduling in `stdlib` / `audio_polish`
//! run before a command is queued, so they behave the same everywhere.
//!
//! Commands are only queued once a shell has called [`enable`]; with no
//! shell (tests, `twec run`) sound is silently skipped, as before.

use std::cell::{Cell, RefCell};

/// One thing for the shell's audio player to do. `path` names the
/// sound as the script did (an asset path; read the bytes with
/// `bundle::read_asset_bytes`).
#[derive(Debug, Clone, PartialEq)]
pub enum AudioCmd {
    Play {
        path: String,
        volume: f32,
        looped: bool,
    },
    /// Stop every playing instance of `path`.
    Stop { path: String },
    /// Set the volume of every playing instance of `path`.
    SetVolume { path: String, volume: f32 },
}

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static QUEUE: RefCell<Vec<AudioCmd>> = const { RefCell::new(Vec::new()) };
}

/// Called by a shell that will [`drain`] and play commands each frame.
pub fn enable() {
    ENABLED.with(|e| e.set(true));
}

pub fn is_enabled() -> bool {
    ENABLED.with(|e| e.get())
}

/// Queue a command if a shell is listening; otherwise drop it.
pub fn push(cmd: AudioCmd) {
    if is_enabled() {
        QUEUE.with(|q| q.borrow_mut().push(cmd));
    }
}

/// Take every command queued since the last call, in order.
pub fn drain() -> Vec<AudioCmd> {
    QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_queue_only_when_a_shell_listens() {
        push(AudioCmd::Stop {
            path: "a.wav".into(),
        });
        assert!(drain().is_empty(), "no shell: dropped");
        enable();
        push(AudioCmd::Play {
            path: "a.wav".into(),
            volume: 0.5,
            looped: false,
        });
        assert_eq!(drain().len(), 1);
        assert!(drain().is_empty(), "drained");
    }
}
