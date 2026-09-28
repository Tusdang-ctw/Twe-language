# web3d-M4 closeout: `survive3d` and the Studio viewport

**Date:** 2026-09-28
**Milestone:** web3d-M4 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md); session plan: [`2026-09-28-web3d-m4-plan.md`](2026-09-28-web3d-m4-plan.md))
**Status:** **codebase-closed; one external step pending.**
- Two of the three exit criteria are met.
- For the third, the itch.io build is ready and checked in Chrome, but publishing it is the maintainer's action.
- Next: M7 (graphics beyond Three.js), then M6.

## Exit criteria

| Criterion | Result | Evidence |
|---|---|---|
| A playable 10-minute run of `survive3d` published on itch.io (web) | **Build ready; not yet published** (maintainer) | See "The itch.io build" below. Checked in Chrome: loads behind the progress screen, plays at 144 fps (display cap), no console errors |
| 10 consecutive full runs, no crash or GC-stress failure | **Met, natively and in wasm** | `cargo test --release --test soak -- --ignored` and `node web/soak.mjs <build> 10 10` (+ `--stress`); numbers below |
| Studio plays the game in its own viewport, with hot reload | **Met** (see the limit noted) | `D:\IT\twe-engine` commit `b831286`; checked in WebView2 153, details below |

### Soak numbers

The scripted player (`twec::soak`):
- moves in a weaving circle;
- picks upgrades, pauses every 90 s, dies and restarts;
- varies its path per session.

It drives the real input path (`InputState` → `sim_tick`) and the 3D render script each tick.

- **Native, release:** 10 sessions of 10 simulated minutes (36,000 ticks each).
  - 3–6 deaths, 27–39 level-ups and up to 597 live entities per session.
  - Heap alive after a full collection at every restart: 32–99 KB, with no growth across runs.
  - Plus 2 minutes with the GC collecting at every safepoint.
  - All clean; 288 s wall for the lot.
- **WebAssembly under Node:** the same 10 sessions and the same stress run.
  - All clean; 11–30 s per 10-minute session.
  - Heap after restarts: 19–64 KB.
- **Native and wasm play identical sessions.**
  - Given the same bot variant, deaths, level-ups and peak entity counts match exactly on both targets.
  - The interpreter, the fixed tick and the seeded RNG are deterministic across targets. This is the property replays and a future netcode rely on.

### The Studio viewport, as checked

- **Tested live** (Studio run with WebView2 remote debugging):
  - `build_viewport` built the project into the app cache.
  - `http://twe.localhost/` (the `twe://` scheme) served it with its CSP.
  - The WebView2 page had a WebGPU adapter, and `survive3d` rendered and played in an iframe inside the Studio window.
  - `project_stamp`, the hot-reload trigger, changed when `main.twe` was touched.
- **Not click-tested:** the Viewport panel's own buttons. They need a project opened through a native dialog, which the automation couldn't drive. The panel is the same iframe plus a rebuild-on-stamp-change poll.

## What shipped (8 commits, `56dd644`..`47c2397`, plus Studio `b831286`)

1. **The game.** `examples/survive3d`:
   - `survive_beta`'s design in 3D (4 enemy types, 3 weapons, XP magnet, waves, a boss every fifth wave), with a level-up picker, pause and restart;
   - an animated hero model and synthesised sound effects;
   - the best wave and time are saved;
   - pauses when the tab or window loses focus;
   - plays with keyboard, mouse or gamepad.
2. **Engine gaps the game found:**
   - scene-state `on render():` in 3D;
   - the full key table on the web;
   - sRGB colour decoding (every 3D scene had looked washed out);
   - `math.clamp` was documented but missing.
3. **Sound in 3D and the browser.** `sound.*` queues commands (`audio_host`); `play3d` plays them through quad-snd, the browser through WebAudio. Web builds ship one `game.twebundle`.
4. **Saves in the browser.** `save.*` and `settings.*` use localStorage through one text-storage layer (`save::write_text` / `read_text` / `exists`) that replays share. `os.data_dir(app)` namespaces keys per game. `TWE_DATA_DIR` overrides the base.
5. **Pause on focus loss** in every 3D shell, through one shared `BlurAutoPause`: window focus in `play3d`; `visibilitychange`, `blur` and `focus` on the web.
6. **The input-command stream** (the net-ready hook). The 3D shells feed a `host3d::InputState`, and each fixed tick takes one `InputCommand` (keys, mouse, gamepad) through the replay recorder.
   - `replay.record` / `play` work in 3D and on the web; format `TWE-REPLAY v2`.
   - `tests/survive3d.rs` replays a recorded 45 s run identically.
   - **Bug fixed on the way:** 3D shells applied input once per rendered frame. At 144 Hz, frames without a tick dropped key presses, and frames with two ticks doubled them.
7. **Mouse and gamepad on the web.**
   - Canvas pointer and wheel, in 640×480 canvas units (the HUD's units; `play3d` too).
   - The Gamepad API, standard mapping; gilrs in `play3d`.
   - A level-up picker driven by arrows / d-pad / hover and confirmed with Enter / A / click.
8. **Asset pipeline.**
   - `tests/gen_survive3d_hero.rs` generates a skinned glTF hero: 5 joints, `walk` / `idle` clips, and a palette texture in a hand-written PNG.
   - `mesh_anim.*` now takes a mesh path. Its integer handle had been impossible for a script to obtain.
   - Web builds name every file but `index.html` after its content (ending the M3 stale-cache problem).
   - Web builds show a titled loading screen with a progress bar over the runtime and the bundle.
9. **Soak harness.** `src/soak.rs`, `tests/soak.rs`, the `soak` wasm export and `web/soak.mjs`.
10. **Studio** (`D:\IT\twe-engine`):
    - a Viewport play mode (the default): the web build in an iframe beside the editor, over `twe://` with a CSP, rebuilt and reloaded when any source or asset changes;
    - native windows stay as fallbacks;
    - the AI loop's prompt now carries the EBNF grammar.

**Tests:** 962 pass. Clippy is clean natively, for wasm32 and for `twe-web`. New programs and tests:
- `tests/survive3d.rs` (play-through, sounds, replay, gamepad, click, saves);
- `tests/soak.rs`;
- `tests/gen_survive3d_{sounds,hero}.rs`;
- three GPU render tests (frame, level-up picker, hero mid-stride);
- the hashed web-build layout;
- input-state unit tests.

## The itch.io build

`target/survive3d-itch.zip` (1.0 MB; regenerate with `twec build --target web --out <dir> examples/survive3d` and zip the folder's contents):
- `index.html` sits at the root;
- every path is relative;
- nothing needs SharedArrayBuffer or special headers.

On itch.io:
1. Create a project with kind **HTML**.
2. Upload the zip and tick **"This file will be played in the browser"**.
3. Set the viewport to 1280 × 960 or larger with fullscreen allowed. The canvas keeps 4:3.

WebGPU is required (Chrome / Edge; Safari 26+; recent Firefox); other browsers get a clear message. **Not verified:** WebGPU inside itch's cross-origin game iframe. Chrome allows it by default, but the first published run is the real check.

## Deferred, with reasons

- **Per-entity animation clips.** `mesh_anim` state is per mesh, so every slime would share one clip. Enemies don't need animation for M4, and a `look:` key for it is a language change that should wait until an example forces it.
- **A GPU-side soak.** The soak runs everything but the GPU. The browser check covered rendering, and a 10-minute live Chrome session is part of the itch.io publish.
- **Replay files leaving the browser.** Recordings live in localStorage; nothing exports them as a file yet.
- **Meshopt / KTX2 asset compression.** The slice's assets total 128 KB, so there was nothing to gain yet. Moves to M7, where real art arrives.

## Doc edits

- `docs/06`:
  - §4.9a: animated meshes;
  - §7.6: mouse units, gamepad in 3D and on the web;
  - §7.11: saves in the browser;
  - §7.14: pause on focus loss;
  - §7.19: the input-command stream.
- `CHANGELOG.md`: M4 entries.
- `CLAUDE.md`: status line.
- `docs/05-roadmap.md`: M4 status.
- `docs/journey.md`: the M4 chapter.
