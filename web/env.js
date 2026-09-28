// web3d-M2: the `env` imports left in the Twe web runtime by macroquad /
// miniquad (the 2D backend, still linked into the interpreter's stdlib
// until web3d-M6 retires it). The WebGPU shell never initialises
// macroquad, so these are only reached by 2D-only builtins; each is a
// safe no-op here. `index.html` maps the bare module name "env" to this
// file with an import map. `tests/web_runtime.rs` fails if the runtime
// ever needs an `env` import this file doesn't provide.

// miniquad's clock (seconds). Must return a number: `undefined` would
// become NaN in an f64 import.
export function now() { return performance.now() / 1000; }

// Audio (web audio lands with the web3d-M4 slice).
export function audio_play_buffer() { return 0; }
export function audio_source_set_volume() {}
export function audio_source_stop() {}
export function audio_source_delete() {}

// miniquad logging (pointers into wasm memory; the shell logs through
// `console` directly instead).
export function console_error() {}
export function console_warn() {}
export function console_info() {}
export function console_debug() {}

// Window / cursor control.
export function sapp_set_cursor() {}
export function sapp_set_cursor_grab() {}
export function sapp_set_fullscreen() {}

// miniquad's file loader (assets are fetched by the shell).
export function fs_get_buffer_size() { return -1; }
export function fs_take_buffer() {}
