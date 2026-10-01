# Changelog

The deprecation log for the public Twe surface. See
`docs/05-roadmap.md` for the phase-by-phase development log; this
file is the user-facing record of what changed between releases.

The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/) once v1.0
ships. Until then, every minor (v0.x) release is permitted to break
the surface, with deprecations rather than removals where the
removal would be load-bearing.

## Unreleased

### Added (benchmark)
- **The Python + pygame-ce baseline** (web3d-M5): `--lang python` on `twec bench grade`, `check` and `run`.
  - The same 20 tasks, the same input replayed through pygame's APIs, and a Python twin of every check.
  - `compile()` and pyflakes play `twec verify`'s part.
  - Setup: `bench/python/requirements.txt`.

### Fixed
- **`examples/tetris.twe` crashed on every hard drop**, and **`grid_duel`, `pong_net` and `pong_net_internet` crashed while waiting for a peer**: each used `return` in an event handler.
- **`twec verify` now reports `return` outside a function or method body**, and a member a list doesn't have (`.len()`). Both used to fail only when the line ran.
- **The LLM primer's examples are real Twe.** The entity example didn't parse, and the events example showed handlers that are only legal inside a state. Every example now verifies, and a test keeps it so.

### Added
- **`twec bench run` / `twec bench regrade`** (web3d-M5): run a model on the benchmark tasks.
  - Each sample gets up to 4 rounds, with `twec verify` and a 2-second smoke run as feedback.
  - Each final program is graded on behaviour.
  - Output: pass@k with bootstrap confidence intervals, error rates, tokens and cost, every program and transcript.
  - Replies are cached, so runs resume and re-grade without new calls.
  - Flags `--rounds 1`, `--no-verify`, `--no-smoke` and `--no-primer` give the ablation arms.
- **`twec bench grade` / `twec bench check`** (web3d-M5): grade a program on a benchmark task by what it does, not by what it prints.
  - The program runs headless for a fixed number of ticks while an input script presses keys; Twe expressions then check the world.
  - Each program is graded in a child process with a time limit.
  - `check` rejects tasks whose checks no broken variant of the solution can fail.
  - Format: `bench/README.md`.

### Changed
- **`twec llm-loop` talks to model APIs** (web3d-M5), in a build with `--features llm-http`:
  - `--provider anthropic --model M [--effort E]`, or `--provider openai --model M [--base-url URL]` for Ollama and llama.cpp; `--command` still works for any program.
  - Rounds are one conversation, and the model answers with SEARCH/REPLACE edits or a whole file.
  - `--starter FILE` edits an existing program; the Twe primer is the system prompt unless `--no-primer`.
  - Traces (version 2) and the summary report tokens and cost.
- **3D renders closer to the path-traced reference** (web3d-M7): energy-conserving diffuse, correct normal-map tangents, iridescence over metals, sharper environment reflections, and glass that no longer shows its back faces. Twe now scores at or below Three.js on all 12 comparison scenes.
- **Large static 3D worlds are much cheaper** (web3d-M7): entities without `update` cost nothing per tick, unchanged `look:` draws are kept instead of rebuilt, GPU occlusion culling switches itself off when it doesn't pay, and many lights shade faster. The stress scene (100k blocks, 500 lights, 1M particles) now runs faster than Three.js's version in Chrome.

### Added
- **`TWE_GPU_PROFILE=1`** prints per-pass GPU times natively; `bench/graphics/compare.sh` compares the stress scene against Three.js.
- **The M7 graphics comparison is published** in `bench/graphics/`:
  - `results.md`, now with a like-for-like column (Twe without AO/SSR) and the scenes where Twe loses;
  - `comparison.jpg`, every scene rendered by Cycles, Twe and Three.js;
  - `fps.mjs`, frame rates in Chrome;
  - `three/stress.html`, the Three.js version of the stress scene.
- **`light.environment(path, intensity)`** lights a 3D scene from an HDR sky image (image-based lighting), and **`camera.far`** sets the 3D view distance (default 100 m, as before).
- **`twec verify` warns when a `particles` block will run on the CPU** (`particles-cpu`), with the reason; a million CPU particles is a slideshow.
- **`survive3d` looks the part** (web3d-M7): a computed sky, sun shadows, torches, ambient occlusion and bloom, a procedural floor, wobbling slimes, glowing gems and spark bursts, at 67 fps on an integrated GPU in Chrome. New `examples/stress_3d.twe`: 100k animated blocks, 500 lights, a million particles.
- **Procedural surface materials** (web3d-M7): a `visual` block can describe a whole surface in code, with no textures. `surface(uv, time, pos, normal) -> material` returns `material(albedo:, normal:, roughness:, metalness:, emission:)`, and an optional `displace(uv, time, pos, normal) -> vec3` moves the mesh's vertices (shadows and lighting follow). `math.clamp`, `mod`, `atan2`, `dot`, `cross`, `length` and `normalize` now work inside visuals, as do `if` expressions. New example: `examples/procedural_materials_3d.twe`.
- **`twec verify` reports `visual` block problems** (`visual-error`), including type errors such as a tuple where a number belongs.
- **Reflections and volumetric fog in 3D** (web3d-M7): `postfx.ssr(strength)` turns on screen-space reflections (glossy surfaces reflect what's on screen); `light.volumetric(true)` lights `light.fog`'s fog per point, with light shafts through the sun's shadows and halos around lights.
- **GPU particles in 3D** (web3d-M7): a `particles` block runs on the GPU in 3D (up to a million particles) when its bodies compile, and on the CPU otherwise; `collide: true` bounces particles off the scene.
- **Hundreds of lights and spot lights in 3D** (web3d-M7): up to 1024 point and spot lights (was 8), shaded through a clustered light grid; `light.cone(handle, direction, angle)` makes a spot light.
- **GPU culling in 3D** (web3d-M7): from 4096 opaque objects up, the GPU culls objects outside the view or hidden behind others (two-phase hierarchical-Z occlusion culling, indirect draws).
- **glTF material extensions** (web3d-M7): `.glb` models render clearcoat, sheen, iridescence, transmission with volume absorption, IOR and specular (`KHR_materials_*`).
- **Translucency and fog in 3D** (web3d-M7): a draw colour with alpha below 1 and glTF `BLEND` materials draw translucent (sorted back to front); `light.fog(density, falloff, color)` adds exponential height fog.
- **Depth of field, motion blur and colour grading in 3D** (web3d-M7): `postfx.dof(focus, f_stop)` (a physical thin lens), `postfx.motion_blur(shutter)` (camera motion) and `postfx.lut(path, strength)` (`.cube` 3D LUTs).
- **Post-processing in 3D** (web3d-M7):
  - `postfx.ao(strength)` / `postfx.ao_radius(r)`: ground-truth ambient occlusion (GTAO) on indirect light;
  - `postfx.exposure(stops)` and `postfx.auto_exposure(true)`;
  - `postfx.tonemap("agx")` and `postfx.tonemap("neutral")` (Khronos PBR Neutral) next to `"aces"` and `"none"`.
- **Shadows in 3D** (web3d-M7): `light.shadow(handle, true)` makes a
  point light cast shadows (up to 4 per frame). Sun shadows are now
  soft (PCSS) and use cascades fitted to the camera.
- **Anti-aliasing in 3D** (web3d-M7): 4× MSAA is always on, and
  `postfx.taa(true)` adds temporal anti-aliasing (jittered frames,
  camera-motion reprojection, neighbourhood clamping).
- **Image-based lighting** (web3d-M7, kernel): an HDR environment map
  lights scenes (GGX-prefiltered specular cube, SH9 irradiance, DFG
  lookup table) and can be drawn as the backdrop. Kernel and harness
  only for now; the script-side setting arrives with `survive3d`.
- **Physically based materials** (web3d-M7): glTF models draw each
  primitive with its own metallic-roughness material — base colour,
  metal-rough, normal, occlusion and emissive maps, texture transforms,
  emissive strength, alpha mask, double-sided surfaces, vertex colours —
  shaded with GGX / Smith / Schlick and multiscatter energy compensation.
  Before, a whole model wore its first material's base-colour texture.
- **Graphics comparison harness** (`bench/graphics/`): Twe and Three.js
  render the Khronos Render Fidelity scenes, scored with FLIP against
  path-traced references.
- **Soak harness** (web3d-M4): `twec::soak` plays a 3D game with a
  scripted player (move, level up, pause, die, restart) through the
  real input and render paths. `tests/soak.rs` runs it natively (a
  1-minute smoke run always, the 10 × 10-minute and GC-stress soaks with
  `--ignored`), and `web/soak.mjs` runs the same driver in the wasm
  build under Node.
- **Animated characters** (web3d-M4): `mesh_anim.play` / `stop` /
  `blend` / `current` take the mesh's path (the same string `look:` and
  `mesh()` use). Before, they took a numeric handle scripts could never
  obtain. `survive3d`'s player is now a skinned model with `walk` and
  `idle` clips, generated by `tests/gen_survive3d_hero.rs`.
- **Web builds: a loading screen and content-hashed files.** The page
  shows the game's title and a progress bar while the runtime and bundle
  download. Every file except `index.html` is named after its content,
  so browsers and CDNs can cache them forever and a new build never mixes
  with old files.
- **Mouse and gamepad in 3D and the browser** (web3d-M4): `mouse.*`
  and `gamepad*` work in `twec play3d` and web builds (the Gamepad API
  on the web) and are part of each tick's input command, so replays
  cover them. `survive3d` plays with a pad (stick, Start to pause, A to
  pick) and its level-up picker takes arrows + Enter or a click.
- **Input-command stream** (web3d-M4): in `twec play3d` and web builds,
  input enters the simulation as one command per fixed tick and passes
  through the replay recorder, so `replay.record` / `replay.play` work
  in 3D and in the browser (logs go to localStorage there). Replay logs
  are now `TWE-REPLAY v2` (adds mouse motion and wheel); v1 logs still
  play.
- **Saves in the browser** (web3d-M4): `save.*` and `settings.*` use the
  page's localStorage in web builds (they were a runtime error). The
  save path is the key; `os.data_dir(app)` returns `app` on the web, so
  one script saves correctly on desktop and in the browser. `survive3d`
  keeps your best wave and time.
- **`auto_pause_on_blur` in 3D and on the web**: `twec play3d` pauses
  on window focus loss, a web build when its tab is hidden or the
  window loses focus. `survive3d` turns it on.
- **`TWE_DATA_DIR`** overrides the base of `os.data_dir` (tests,
  portable installs).
- **Sound in 3D and in the browser** (web3d-M4): `sound.*` plays in
  `twec play3d` and in web builds (WebAudio, unlocked by the first key
  press or click). `survive3d` has synthesised effects for shots, hits,
  pickups, damage, level-up and the boss.
- **Web builds ship one `game.twebundle`** instead of loose files: the
  shell fetches and mounts it, so sounds, meshes and textures load the
  same way as in a desktop `.exe`.
- **`examples/survive3d`** (web3d-M4): the v1.0 slice, a top-down 3D
  survivors game with enemies, auto-weapons, XP, level-ups, a boss every
  fifth wave, pause and restart, playable in Chrome and natively.
- **`math.clamp(v, lo, hi)`**: documented since Phase 9, now actually
  installed.
- **3D scene states draw**: a scene state's `on render():` now runs in
  3D (it only ran in 2D), so per-state HUDs and menus work.
- **Full keyboard on the web**: letters, digits, F-keys and modifiers,
  the same names as native.
- **HUD text in 3D** (web3d-M3): `text()` and `rect()` inside a 3D
  `on render():` draw screen-space text and boxes over the scene, in the
  2D canvas's 640×480 coordinates. They used to be an error in 3D.
- **Correct colour in the browser**: the web build now renders through
  an sRGB view of the canvas, so browsers that give WebGPU a non-sRGB
  canvas no longer show the scene without its final gamma curve.
- **`look:` `material`** (web3d-M3): a `visual` block as a mesh's
  surface — its `pixel(uv, time)` shades the mesh on the GPU, lit and
  tinted, with alpha below 0.5 cut out. Visual blocks now reach the
  3D game, not only the fullscreen `twec play_visual` viewer.
- **`look:` `facing`** (web3d-M3): yaw in radians about +Y (0 faces +Z),
  applied on the GPU to the mesh and its shadow; and **`math.atan2(y, x)`**
  to turn a direction into a facing (`facing: math.atan2(dx, dz)`).
- **`look:` blocks** (web3d-M3; `docs/06` §4.9a). In 3D, an entity
  declares how it is drawn (`mesh`, `tint`, `scale`) and the runtime
  draws every live instance at its `pos`, with no per-entity draw code.
  On the 5,000-enemy `swarm_3d` benchmark in Chrome, script-side
  drawing fell from ~11 ms to ~1 ms per frame, and the frame rate rose
  from 39–49 to 114–127 fps. `twec verify` checks the keys (with a
  rename fix for typos). The 2D player refuses `look:` with an error
  until web3d-M6.
- **3D games run in the browser on WebGPU** (web3d-M2, in progress). The
  new `crates/twe-web` shell runs a Twe script in the page and draws it
  through the same renderer kernel as `twec play3d`; `examples/hello_3d.twe`
  renders in Chrome at the display's refresh rate. The renderer now fits
  WebGPU's default limits (4 bind groups) and is on wgpu 30.
- **`twec build --target web`** (web3d-M2) writes a servable folder: the
  prebuilt WebGPU runtime, `index.html`, the script and its `assets/`. No
  Rust toolchain needed on the author's machine. Replaces the placeholder
  `wasm32-3d` target (kept as an alias). Single-file games only for now
  (`import` reads modules from the filesystem).
- **`frame_stats()` in the web runtime** (web3d-M3): per-frame averages
  (ticks per frame, update ms, script-render ms, kernel ms) for measuring
  a game in the browser. `examples/swarm_3d.twe` is the 5,000-enemy
  benchmark M3 is measured on.
- **`twec build` accepts a single `.twe` file** as well as a project
  directory: the game is named after the file, with its folder's
  `assets/`.
- **`quit()` and `quit_on_escape(flag)`** (web3d-M0; `docs/06` §7.14a). Games
  can now exit from their own menus, and can take Escape over for a pause
  menu. Previously Escape was hard-wired to close the window (so a script's
  Escape handler never ran) and there was no way to quit from script.
  `examples/pause_menu_demo.twe` and `examples/survive_beta` use both.
- **Dimensional checking in strict mode** (Principle 3, "dimensional units
  enforced"): `+` / `-` between quantities with incompatible units now
  raises in strict mode / `twec verify` — `5m + 3s` reports `dimensional
  mismatch — cannot add \`m\` and \`s\``. Same-unit add (`5m + 3m`) and
  unit-combining `*` / `/` (`m*s`, `m/s`, unit cancellation) are legal. The
  default non-strict tier stays silent (no false positives). See `docs/06`
  §5.5.

### Changed
- **3D rendering is faster:** untextured cubes and spheres skip the glTF material path (about 2.5× cheaper on an integrated GPU), ambient occlusion runs at half resolution, looks of entities that don't change aren't regathered every frame, and a frame with 100k material draws no longer spends 90 ms checking pipelines.
- **Colours from a `visual` material are sRGB**, like every other colour a script writes; a material's midtones render slightly darker than before. Unknown method names in a `visual` block are now an error instead of being ignored.
- **Particle randomness** (web3d-M7): `random.float()` inside `on_spawn` / `on_update` now draws from the emitter's own random stream, so spawning particles no longer changes the script's random numbers. In 3D, a particle's default `size` is 0.1 (a radius in world units).
- When more than four lights ask for shadows, the four nearest the camera get them (it was the first four added).
- Native 3D picks the discrete GPU on machines with two (it picked the integrated one); `TWE_GPU_POWER=low` asks for the integrated GPU.
- `postfx.bloom` is now a multi-level bloom chain (Jimenez 2014) instead of a 12-pixel inline kernel: the glare reaches much further with a soft falloff. `postfx.tonemap` takes a curve name; `true` / `false` still mean ACES / none.
- `sun.shadow_extent(r)` now means shadows reach `4 × r` from the camera,
  including casters up to `r` outside the view (it used to be the radius
  of a fixed square around the camera target).
- **3D tone curve and shading (visible change).** ACES now uses the
  fitted RRT+ODT curve Three.js and the glTF references use (was a
  darker approximation), and cubes / spheres shade as a physically based
  dielectric, with specular highlights.
- **`mouse.x` / `mouse.y` in 3D** are in 640×480 canvas units (as in
  2D and the HUD), not window pixels (web3d-M4).
- **3D colours are sRGB (visible change).** Tints and `cube()` /
  `sphere()` colours are now decoded from sRGB before lighting, as
  authors pick them (Three.js does the same). Dark colours look dark
  and saturated ones saturated; scenes used to look washed out.
- **`look` is a reserved word (breaking; web3d-M3).** A variable named
  `look` must be renamed. `examples/fps_demo.twe` had one.
- **`twec play` exits with status 1 when the script fails to start**
  (read, parse or top-level error). It used to exit 0.
- **Lexical scoping (breaking; web3d-M1,
  `docs/changes/2026-09-28-web3d-m1-lexical-scoping.md`).** Function /
  method / handler bodies have their own locals; a callee can't see its
  caller's parameters; locals don't leak after a call; a name declared in a
  block isn't visible after it; module functions resolve names in their own
  module. Assigning an undeclared name and re-declaring a visible name with
  `let` / `var` are errors. Scope errors are reported before a program runs
  and by `twec verify`. No program in the examples or test corpus depended
  on the old dynamic behaviour.
- **Scaffolding namespaces are now behind `--features experimental`**
  (web3d-M0; `docs/changes/2026-09-27-web3d-pivot.md`): `console.*`,
  `achievements.*`, `cloud_save.*`, `friends.*`, `mmo.*`, `workshop.*`,
  `rollback.*`, `world.*` and `terrain.*`. Their runtimes were stubs or
  bookkeeping the renderer never consumed. They no longer appear in the
  default build, `twec stdlib --json`, or the LLM corpus. Scripts using them
  get an "undefined name" error unless built with
  `cargo build --features experimental`. Their demos moved to
  `examples/experimental/`. `net.*` (lockstep, lobbies, reconnect,
  `net.set_mode`) is unchanged.
- 2D drawing and UI builtins (`rect`, `text`, `button`, …) called outside the
  2D runtime (`twec play3d`, or headless) now raise
  `… is a 2D drawing call and needs the 2D runtime` instead of crashing the
  process. Touch queries report no touches headless.

### Fixed
- Built-in `sphere()` meshes were wound inside-out, so since back-face culling arrived (web3d-M7 session 3) every sphere drew its far inner hemisphere: dark, with a lit rim.
- **glTF models without normals** are now shaded with flat face normals,
  as the glTF spec requires; the loader used to point them all straight
  up, lighting every such surface like a floor (web3d-M7).
- **Lost and doubled key presses in 3D** (web3d-M4). The 3D shells
  applied input once per rendered frame: on a high-refresh display a
  frame that ran no simulation tick dropped `key_press`, and one that
  ran two saw it twice. Each press now reaches exactly one tick. Keys
  held when the window loses focus are released instead of sticking.
- **`twec play3d` no longer crashes on its first frame.** Since 2026-06-01 the
  3D loop called macroquad's clock, which panics outside a macroquad window,
  so every 3D example aborted immediately. A source-level test now keeps
  macroquad out of the 3D shell, and a headless render test
  (`tests/kernel_render.rs`) renders `hello_3d` through the real pipeline.
- **GC use-after-free** (web3d-M0; `docs/changes/2026-09-27-web3d-m0-gc-soundness.md`):
  a `for` loop over a temporary list (e.g. a list comprehension) could
  corrupt the heap and crash (`STATUS_HEAP_CORRUPTION`) once the body
  allocated enough to trigger a collection. Collections no longer run over
  unrooted temporaries. The incremental sweep, module cache, stdlib stores
  and class parent chains are now handled correctly. New `TWE_GC_STRESS=1`
  mode and `tests/gc_stress.rs` gate this.
- The pause flag (`pause()`, `auto_pause_when_idle`, `auto_pause_on_blur`) is
  per interpreter thread instead of process-wide.
- **`import` works in `twec run <file>`, `twec play` and `twec play3d`.**
  Only `twec run <dir>` loaded modules — and it never ticked frames — so
  every other way of running a script left imports unbound (the Phase 13
  module demos failed as soon as an `on update` touched an imported
  module). All of them now share one loader (`module::prepare_entry`).
- **Project-relative asset paths work from any directory.** `twec run` /
  `play` / `play3d` / `play_visual` register the script's directory as the
  asset root; a `load("assets/hero.png")` that isn't found relative to the
  working directory is looked up there — the same key a shipped bundle uses.
- `joystick(at: (x, y), …)` accepted only lists, so the documented tuple
  form always errored.
- Broken examples, found by the new lexical resolver and the new
  every-example CI test (web3d-M1): undefined speaker in `rpg_demo`, the
  nonexistent `list.length` / `rgb` / `nil` / `str` / `draw_*` calls in
  `pong_net_internet`, `rhythm_demo`, `survive_beta_mobile` and
  `survive_demo`, and a seconds-plus-float unit error in `atlas_demo`.
  Both module demos (`modular_audio_demo`, `modular_math_demo`) failed too:
  see the `import` entry below.
- **Shipped builds load their bundled assets.** `load`, `load_atlas` and
  `sound.load` checked the loose filesystem before the bundle, so a bundled
  `.exe` on a machine without the `assets/` folder failed on assets it
  carried. They now resolve through the bundle first (`bundle::asset_exists`).
  On the web target the check defers to the async loaders instead of always
  failing.
- **The browser (wasm32) build compiles again.** It had silently broken
  during the 3D phases (32 errors). 3D-only state types moved to
  `render3d_types`; `physics.*` is not installed on wasm32 (rapier is
  native-only). CI now runs clippy for wasm32. Web saves never worked:
  they wrote to URL query parameters, not localStorage, and the load half
  didn't compile. They now return a clear "not supported in the browser yet"
  error until real localStorage lands (web3d-M2), and the `quad-url`
  dependency is removed.
- **Steam initialises in shipped builds.** The embedded (bundled-exe) play
  loop skipped `steam::init()`, so achievements and cloud saves silently did
  nothing in a Steam build.
- Strict-mode arithmetic no longer reports a false `type mismatch` when an
  operand is of **unknown type** (e.g. an element of an untyped iterable,
  `for x in items: s = s + x`). The mismatch now fires only when *both*
  operands are resolved concrete types — an Unknown/unbound operand can't
  be proven non-numeric ("no false positives"). Genuine concrete mismatches
  (`str + int`, `5m + 3s`) are unchanged. This is the root cause behind the
  record-field symptom below.
- Strict-mode field access on a **record-typed** value (e.g. a parameter
  annotated `{ x: int, y: int }`) now resolves to the declared field
  type. Previously `p.x` inferred `Unknown`, so using it — e.g.
  `p.x + p.y` — raised a spurious `? vs ?` arithmetic mismatch even when
  the call type-checked via width subtyping. Width subtyping (a value
  missing a required field still fails the call) is unchanged.
- Type inference now resolves **mutually-recursive** (and forward-
  referenced) top-level functions. `walk_program` does a two-pass scan —
  pre-register every top-level function signature, then walk bodies — so
  `is_even` calling a later-declared `is_odd` no longer raises a spurious
  strict-mode / `twec verify` "unknown name" error (it ran fine at
  runtime; this was a false positive in the strict/verified tier).
  Genuinely-undeclared names still error.

### Added
- `then` sequencing keyword (Example 10): `<action> then <body>` evaluates
  the action (an expression yielding a duration), waits that long, then
  runs the body. Reuses the `wait` fiber machinery, so — like `wait` — it
  only suspends inside a state on-entry body. Tree-walker only (the frozen
  bytecode VM rejects it). See `docs/06` §4.8.
- List comprehensions (Snake NP3): `[<elem> for <var> in <iter>]` with an
  optional `if <cond>` filter. Iterates ranges / lists / tuples; the loop
  variable is scoped to the comprehension. Tree-walker only (the frozen
  bytecode VM rejects it at compile time). `infer` types it as
  `List<element-type>`. See `docs/06` §6.1.
- State lifecycle hooks `on enter:` / `on exit:` (Snake NP9). `on enter:`
  folds into the existing on-entry body (one entry mechanism; works on
  both backends). `on exit:` runs when a state is left, before the next
  state's entry — the cleanup counterpart to entry code (tree-walker
  only; the bytecode VM rejects it at compile time). See `docs/06` §4.8a.
- `rect_outline(at, size, thickness, color)` — the outline counterpart to
  `rect`, mirroring `circle` / `circle_outline`. Finalizes the v1.0
  drawing-primitive set (see `docs/06` §7.5); `triangle` / `polygon` /
  `arc` / `ellipse` are deliberately excluded as composable / unused.
- `physics2d.*` — hand-rolled, std-only 2D collision + motion (no
  `rapier2d` dependency). Narrow phase: `overlap(a, b)`, `resolve(a, b)`
  (minimum translation vector), `sweep(box, vel, solids)` (swept-AABB
  time-of-impact + normal), `circle_overlap`, `circle_hits(center, r,
  boxes)`. Broad phase: `broadphase(boxes, cell)` builds a spatial-hash
  grid (handle), `grid_query` / `grid_near` return sorted candidate
  indices, `grid_free` releases it. Dynamics: `move_and_slide(box, vel,
  dt, solids)` integrates a body against static solids with swept
  collision + sliding, returning `{x, y, vx, vy, on_ground, on_ceiling,
  on_wall, hit}` (stateless; the script owns the body). Boxes are
  `(x, y, w, h)` top-left tuples. Rigid-body-lite: `bounce(vel, normal,
  restitution)` reflects a velocity off a static surface, and
  `collide(p1, v1, m1, p2, v2, m2, restitution)` resolves a mass-weighted,
  momentum-conserving two-body collision → `{v1x, v1y, v2x, v2y}`. Angular
  dynamics + joints remain out of scope. `examples/platformer_physics2d.twe`
  demonstrates `move_and_slide`.
- `os.data_dir(app)` — returns the platform-correct, per-user, writable
  directory (`%APPDATA%\<app>` on Windows, `~/Library/Application Support/
  <app>` on macOS, `$XDG_DATA_HOME`/`~/.local/share/<app>` on Linux; `""`
  on WASM), creating it on first call. Gives a shipped game a safe place
  to write saves/settings when its bundle is mounted read-only. Std-only,
  no new dependency.

## v1.0.2 — Deferral-debt patch (closed 2026-05-26)

> Patch-tier release after v1.0.1 that closes the structural half
> of two long-open `What is open` items plus four cross-phase
> deferrals from Phases 13 / 32 / 37 / 39. Every retained session
> is pure parser sugar over existing builtins or one small additive
> runtime hook. Full plan in
> [`docs/v1.0.2-plan.md`](docs/v1.0.2-plan.md); closeout at
> [`docs/changes/2026-05-26-v1.0.2-closeout.md`](docs/changes/2026-05-26-v1.0.2-closeout.md).
> Session 3 (`entity X: lod` / `rollback` parser sugar) was cut at
> the planned spike — runtime targets aren't ready; defers to v1.1
> alongside Phase 32 wgpu render integration + Phase 37 eval-side
> rewind engine. **Net +13 tests; 1004 passing.** Zero new public
> builtins, zero new AST variants.

### Added

- **`save SaveSlot:` block + `migration from N:` clauses** (Session 1,
  Path B anchor-only). Pure parser sugar over the v1.0.1 stateless
  schema-version primitives. `version: N` declares the current
  schema; each `migration from M:` body runs when
  `save.loaded_version() ∈ {1..M}` and `M < N`. Closes the
  structural half of the v1.0.1 Session 5 deferral; typed-field
  Path A remains v1.1 work. See
  [`docs/07-save-system.md`](docs/07-save-system.md) §What v1.0.2
  implements.
- **`state X: pause: false` / `state X: persistent` parser sugar**
  (Session 2). Lowers to the v1.0.1 `persistent_state(name)`
  registry. `persistent` is an alias for `pause: false`; both forms
  inject a top-level `persistent_state("X")` call after the
  enclosing declaration. Closes the v1.0.1 Session 6 parser-sugar
  deferral.
- **`lang.set_plural_rule(locale, fn)` accepts Twe closures**
  (Session 4). Custom plural rules `(n: int) -> string` for the
  long-tail locales the CLDR built-ins don't cover. Closes the
  v1.0.1 Session 12 alias-only deferral. Exposes
  `eval::call_function` as `pub(crate)` for stdlib callback paths.
- **`twec run <dir>` auto-detects `main.twe`** (Session 6). Routes
  through the module loader; multi-file projects work without
  `/main.twe` on the command line. Closes the Phase 13 closeout
  deferral.
- **`touch.tap_count` play-loop hook** (Session 7). Sliding-window
  detector — taps held <250ms count, window is 500ms.
  Tap-press / tap-release diff runs once per frame against
  `macroquad::input::touches()` from the play loop. Closes Phase 39
  deferral #5.
- **MSG_HELLO mode-mismatch handshake check** (Session 8). 1-byte
  mode field in the MSG_HELLO payload; mismatched peers receive
  `MSG_BYE` from the host and a clean error from the client side.
  Closes Phase 37 deferral #4. Backward-compatible: pre-v1.0.2
  4-byte hello payloads still accept (assume same mode).

### Fixed

- **LSP cross-module rename no longer false-positives inside
  triple-quoted strings** (Session 5). `find_occurrences` now uses
  a lexer-driven scan instead of a per-line byte scan; identifier
  tokens come from the lexer, so string contents are never matched
  regardless of how many lines they span. The byte-scan path is
  retained as a fallback for documents the lexer rejects.

### Internal

- **VM-mirror parity test for `world.*` + `terrain.*` builtins**
  (Session 9). The 35 builtins were already reachable from the
  bytecode VM via `VM::new()` → `stdlib::install` → globals copy;
  the test pins the path so a future regression that shadows the
  `world` Object trips here rather than at release-tag time.
- **`docs/06-design-document.md` §7.20 writeup** for the
  `world.*` + `terrain.*` namespaces (Session 10) with worked
  examples from `examples/openworld_demo.twe`. Closes the Phase 32
  doc deferral.
- **EXIT GATE: `examples/survive_beta/main.twe` migrated onto
  `save SaveSlot:`** (Session 11). LOC delta +2 — the block-header
  fixed cost, accepted in the plan's honest exit-criterion
  revision. `tests/programs/v1_0_2_sugar.twe` exercises both
  shipped sugar paths end-to-end.

### Notes

- Session 3 (`entity X: lod = [...]` / `entity X: rollback = true`
  parser sugar) was cut at the planned 30-minute spike. Tuple-with-
  named-fields isn't real Twe syntax, the rollback runtime has no
  per-entity hook, and both halves are defer-on-defer over
  phase-sized runtime work. Re-enters in v1.1 alongside its
  respective runtime follow-on.
- No new keywords: `save`, `version`, `from`, `migration`,
  `persistent`, `pause` all stay as contextual idents recognized by
  the parser. The Phase-35 API stability snapshot
  (`docs/api-snapshots/2026-05-10-baseline.json`) sees only
  additive surface.

---

## v1.0.1 — Polish release (closed 2026-05-18)

> Patch-tier release after v1.0 that hardens the Survivors-class
> path: game feel as one-call procedural effects, audio polish, 2D
> dynamic lighting, save migrations, contributor LSP, replay-on-crash,
> and CI perf snapshots. Full plan in [`docs/v1.0.1-plan.md`](docs/v1.0.1-plan.md);
> closeout at [`docs/changes/2026-05-18-v1.0.1-closeout.md`](docs/changes/2026-05-18-v1.0.1-closeout.md).
> No cloud-hosted assets — fully procedural fx/lighting libraries
> instead, for the determinism + offline-`.exe` + LLM-grounding
> reasons enumerated in the plan. Net **+53 tests; 991 passing**
> (includes closing the 12 pre-existing CRLF-cascade failures via
> a one-line lexer fix).

### Added

- **`fx.*` procedural VFX library** (Session 1, 2026-05-12).
  Twelve call-and-go effects covering the standard Survivors-class
  hit-feedback vocabulary, all procedural — no PNGs, no shaders,
  no asset CDN:
  - `fx.hit_flash(at, size, color, duration)` — tint flash over a sprite rect
  - `fx.screen_shake(amount, duration)` — canonical screen-shake (`camera.shake` kept as a back-compat alias; shares state)
  - `fx.hit_stop(duration)` — freeze gameplay for N seconds (counts in physics ticks, replay-safe)
  - `fx.damage_number(at, value, color)` — rising fading number
  - `fx.crit_text(at, value)` — bigger yellow crit text
  - `fx.death_burst(at, count, color)` — radial particle explosion
  - `fx.pickup_pop(at, color)` — expanding outlined circle
  - `fx.dash_trail(at, color)` — call per-frame to leave a streak
  - `fx.level_up_ring(at, color)` — expanding ring
  - `fx.blood_splat(at, dir, color)` — directional cone splatter
  - `fx.muzzle_flash(at, dir)` — gunfire flash
  - `fx.ground_shockwave(at, radius)` — white expanding ring

  Reference: [`examples/fx_demo.twe`](examples/fx_demo.twe). Documented in
  [`docs/06-design-document.md`](docs/06-design-document.md) §7.8b.
  All four play-loop variants wired (tree-walker `run_loop` /
  `run_loop_wasm` / `run_loop_embedded`, bytecode VM `run_loop_bytecode`).
  +4 unit tests. **942 tests pass.**

- **`tween.*` deterministic easing primitives** (Session 2,
  2026-05-13). Pure functions of `t` — replay-safe by construction:
  `tween.ease(name, t)`, `tween.lerp(a, b, t)`, `tween.lerp_eased`,
  `tween.bounce`, `tween.shake`, `tween.eases()` enumerates the
  fourteen supported curves.

- **`light2d.*` dynamic 2D lighting** (Session 3, 2026-05-14).
  Cheap additive multi-light pass with optional AABB shadow caster.
  `light2d.add(at, color, radius, flicker)`, `light2d.set_ambient`,
  `light2d.cast_shadows`, `light2d.clear`. 16-light budget per
  frame. Reference: `examples/dungeon_demo.twe`.

- **Audio polish — pooling + ducking + music layers** (Session 4,
  2026-05-14). `sound.pool("path", max_voices: N)` lifts the
  per-`play` voice limit; `sound.duck` ducks a channel while a
  triggered sound plays. New `music.*` namespace: `music.layer`
  (weighted blend), `music.crossfade`, `music.stop`. Reference:
  `examples/audio_demo.twe`.

- **Save schema versioning (MVP)** (Session 5, 2026-05-14). Three
  builtins on `save.*`: `set_schema_version(n)` stamps the in-memory
  store; `schema_version()` reads it; `loaded_version()` reads what
  the on-disk save was stamped with. Scripts branch on the loaded
  version to run their own migration logic. **Honest scope reduction:**
  the language-level `save SaveSlot:` block + `migration from N:`
  sub-blocks defer to v1.0.2 (needs lexer / parser / AST work).
  Reference: `tests/programs/save_schema_version.twe`.

- **Per-state pause opt-out (MVP)** (Session 6, 2026-05-15). Stdlib
  registry of "persistent" state names: `persistent_state(name)` /
  `clear_persistent_state(name)` / `clear_persistent_states()` /
  `is_persistent_state(name)`. The eval / VM pause filter walks the
  registry and keeps registered states ticking while the global
  pause flag is set, so debug overlays / pause menus / toast HUDs
  keep running. **Honest scope reduction:** parser-sugar form
  (`state X: pause: false` / `state X: persistent`) defers to v1.0.2.
  The MVP closes the *functional* `CLAUDE.md` "What is open" item.

- **Nine-slice / nine-patch panels** (Session 7, 2026-05-15).
  `panel(at, size, skin: nine_slice("path", border: N))` lets the
  Phase 10 widget set render skinned panels. Solid-color fallback
  preserved.

- **`camera2d.*` follow + zoom + cinematic pan** (Session 8,
  2026-05-15). Survivors-class follow camera in a one-liner:
  `camera2d.follow(entity, lerp, deadzone)`, `camera2d.zoom_to`,
  `camera2d.cinematic_pan`, `camera2d.bounds`. `examples/survive_beta`
  rewrites its hand-rolled follow logic against the new API.

- **LSP cross-module find-references + rename** (Session 9,
  2026-05-16). Phase 13 modules + Phase 3/13 LSP now support
  cross-`import`-boundary go-to-definition, find-references, and
  rename refactor (multi-file safe; word-boundary scan skips
  strings + `#` comments).

- **Replay-on-crash + `twec replay`** (Session 10, 2026-05-16).
  Always-on input ring stores the last 30 seconds of frames; the
  crash reporter writes a sibling `twec-crash-<secs>-<pid>.replay`
  next to every `.log`. New CLI subcommand `twec replay <script>
  <replay-file>` re-runs the bug.

- **CI perf snapshot + `twec perf-snapshot` / `twec perf-diff`**
  (Session 11, 2026-05-16). New `.github/workflows/perf.yml` runs
  `cargo bench --bench vm` on push-to-main, scrapes criterion's
  `target/criterion/` into a deterministic JSON document, and
  diffs against the checked-in
  `docs/perf-snapshots/v1.0.1-baseline.json`. Default 5% regression
  threshold fails CI.

- **Localization plurals** (Session 12, 2026-05-18). CLDR-style
  cardinal plural rules for **en / es / de / ja / pl** plus ten more
  Steam-relevant locales (fr / it / nl / pt / sv / no / da / zh /
  ko / th / vi / ru / uk). `lang.t_plural(key, n, args)` selects a
  `<key>.<one|few|many|other>` template; `{n}` substitutes the
  count, `{0}+` substitutes positional args (same shape as
  `lang.tf`). `lang.plural_category(locale, n)` exposes the rule
  directly; `lang.set_plural_rule(locale, base_locale)` aliases
  long-tail locales onto a built-in rule (e.g. `pt-BR` → `es`).
  Closes the third `CLAUDE.md` "What is open" item.

- **`twec doctor`** (Session 13, 2026-05-18). Triage diagnostic
  command. Reports twec version + target triple + active feature
  flags + effective crash directory + last 3 crash logs + cache
  directory (via `$TWEC_CACHE_DIR`). `--json` for the
  LLM-grounded support workflow; `-o PATH` writes to a file.
  Always exits 0.

### Fixed

- **CRLF blank-line indent tracker** ([src/lexer.rs](src/lexer.rs)).
  `handle_line_start` now treats a lone `\r` (Windows blank line)
  as a blank-line marker, alongside `\n` / `#` / EOF. Without this,
  40 of 53 `examples/*.twe` files on Windows checkouts tripped the
  parser with a phantom column-0 Indent token at the next non-blank
  line. Closes the 12 pre-existing CRLF-cascade test failures that
  had carried through v1.0.

### Changed

- **`sound.pool` accepts a string path** in addition to a loaded
  handle. The plan's documented call shape is `sound.pool("sfx/
  hit.wav", max_voices: 8)`; the previous implementation only
  accepted a `sound.load(...)` handle, which can't run at top level
  before macroquad initialises. Pool is voice-budget declaration —
  the asset doesn't need to exist yet.

- **`examples/survive_beta/main.twe` rewritten to use v1.0.1 polish
  APIs.** All four damage sites funnel through a `take_player_damage`
  helper that calls `fx.hit_flash` / `fx.screen_shake` /
  `fx.damage_number`; hand-rolled camera-clamp replaced with
  `camera2d.bounds` + `camera2d.follow(deadzone: (60, 40))`;
  boss-arrival shake + ground shockwave; `save.set_schema_version(1)`
  + `sound.pool(...)` declared at top level. **Net: 1300 → 1286 LOC
  (-14).** Closes Exit Criterion 5 in [`docs/v1.0.1-plan.md`](docs/v1.0.1-plan.md).

### Closeout (Session 14)

See [`docs/changes/2026-05-18-v1.0.1-closeout.md`](docs/changes/2026-05-18-v1.0.1-closeout.md).

---

## v0.1.0 — First public release (2026-05-07)

The first public-tagged release of Twe. Everything below the line
ships in this build; what's open and tracked in CLAUDE.md / the
roadmap is not in scope here.

### Highlights

- **2D runtime** (`twec play`) — macroquad-backed game loop, full
  UI widget set (button, slider, dropdown, panel, stack, flex,
  grid, scroll, text input, key-input rebind), pause stack,
  settings + localization, gamepad, particles, clipboard, hot
  reload, screenshot (F12), frame-time HUD (F3), crash reporter.
- **3D runtime** (`twec play3d`) — wgpu pipeline with rapier3d
  physics, glTF 2.0 multi-node scene flatten + GPU skinning +
  animation channel sampling, 8 point lights + Blinn-Phong, 2K
  shadow maps with 3×3 PCF, HDR linear lighting + ACES filmic
  tone mapping + vignette, frustum culling, dynamic instance
  buffer, distance-attenuated 3D audio, KinematicCharacterController
  with raycasts + collision events, typed `save.*` namespace.
- **Visual runtime** (`twec play_visual`) — `visual` blocks
  compile to WGSL fragment shaders.
- **Build pipeline** (`twec build`) — produces a self-extracting
  Windows `.exe` with the bundled game + Twe runtime; macOS
  `.app` and Linux `.AppDir` directory layouts also supported
  (per-target binaries via cargo-dist in this release).
- **Module system** + **strict mode v2** + **verified-mode JSON
  diagnostics** for LLM tool-use loops.
- **Tooling** — `twec fmt` (trivia-preserving since Phase 27),
  `twec verify`, `twec types`, `twec profile` (Chrome trace),
  `twec info`, `twec bench` (criterion-based), tree-sitter
  grammar, LSP with hover + completion + go-to-definition.
- **737 tests pass** across lib + 12 integration binaries.
- **Two reference games**: `survive_beta` (Vampire-Survivors
  clone, ~1300 lines) and `crystal_hunter` (3D FPS, ~250 lines).

### Released artifacts

Cross-platform binaries attached to the GitHub Release for:

- `x86_64-pc-windows-msvc` (`.zip`)
- `x86_64-unknown-linux-gnu` (`.tar.gz`)
- `x86_64-apple-darwin` (`.tar.gz`)
- `aarch64-apple-darwin` (`.tar.gz`)

Each archive contains the `twec` binary plus README, LICENSE,
and CHANGELOG.

### Known limitations carried into v0.1

- Bytecode VM is partial: rejects `on render():`, keyword
  arguments to builtin calls, dialogue, and non-literal field
  defaults. The tree-walker is the canonical execution path;
  `--vm bytecode` falls back with a clean compile error.
- The bytecode VM is currently 1.1×–1.8× *slower* than the
  pre-NaN-tag baseline on tight integer loops; the 3× target is
  unmet but the criterion harness (`benches/vm.rs`) is in place
  to drive it down.
- Auto-pause-on-window-blur ships only on Windows;
  macOS / X11 / Wayland focus paths stub `is_focused() = true`.
- Cross-compiled per-target twec runtimes for the macOS `.app`
  and Linux `.AppDir` layouts produce empty shells today; the
  cargo-dist release pipeline (this release) fills them in.

## v0.7 (Phase 13) — Modules + type-system stability

**Status:** in development.

This is the public-API freeze that v0.8+ depends on. Anything
flagged with `@deprecated("since v0.7")` here will keep working in
v0.7.x and v0.8 (a 12-month carry-over per
`docs/05-roadmap.md` §"Phase 13"), then be removed in v1.0.

### Added

- **Module / package system.** `import "<path>"` and
  `import "<path>" as Alias` bind a module value whose fields are
  the imported file's top-level names. Multi-file projects are
  supported out of the box; the importer's directory is the
  default search path.
- **`twe.toml [dependencies]`.** Each entry maps a logical name to
  a search path (table form) or a version pin (string form). The
  resolver consults dependency paths before the importer's directory.
- **Strict mode v2.** Structural-record subtyping (`{x: int, y: int}`)
  and Luau-style lax narrowing (a Union → variant assignment is
  accepted as an implicit narrowing assertion).
- **Verified mode (Tier 3).** `# verified` directive + the
  `twec verify <file>` subcommand emit a JSON document an LLM can
  sit in a self-correction loop with.
- **`@deprecated("since vX.Y")` annotations.** Attach to top-level
  function and type declarations. `twec verify --warn-deprecated`
  surfaces a `deprecation` warning per use site.

### Deprecated (web3d-M5)

- **`twec eval`**, in favour of `twec bench`. It graded by stdout after N frames with no input; `twec bench` grades by behaviour under scripted input. It still works and prints a note.

### Deprecated (since v0.7)

(None yet — first cycle. As the surface evolves through v0.7.x and
into v0.8, additions here document the 12-month-carry-over schedule
each retired symbol is on.)

### Changed

- The `# strict` directive's behavior: structural records and
  Union-to-variant lax narrowing are now part of the strict
  contract. Programs that relied on strict rejecting these will
  see fewer diagnostics. No source-level breakage — the change is
  purely "fewer errors in strict mode."

### Removed

(None.)

---

Earlier phases (v0.1 — v0.6) are tracked in
`docs/changes/` as per-session closeout notes; this file picks up at
v0.7 because the API-freeze contract is what users care about, and
that's a Phase 13 concern.
