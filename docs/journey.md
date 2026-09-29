# The Twe journey

A dated record of how Twe was built, from the first commit on. It's written as source material for anyone telling Twe's story, whether an article, a talk or a launch post, so it keeps the setbacks alongside the wins.

Every entry points at its evidence: the closeout notes in [`changes/`](changes/) and the git history. Numbers are measured, never estimated, and each one names what it measured.

**Maintenance rule:** add an entry whenever a milestone closes or something notable happens, in the same commit. Don't rewrite history; add corrections as corrections.

---

## At a glance (as of 2026-09-28)

| | |
|---|---|
| First commit | 2026-04-27 (`phase-0: lock in design docs`) |
| Commits | 274 |
| Rust source | ~66,000 lines |
| Tests | 940, real Twe programs plus unit tests, all green |
| Example games / programs | 41 single-file + 12 multi-file projects |
| Design and closeout notes | 76 in `docs/changes/` |
| Built by | one developer working with Claude (Anthropic's AI model) through Claude Code |

---

## Timeline

### Act I: a language in a week (2026-04-27 → 2026-04-29)

- **Day 1, 2026-04-27.**
  - The design documents were locked before any code: five principles in strict priority order ("game concepts are first-class", "one obvious way", "no silent footguns", "AI-legible by design", "engine-native"), plus ten example programs declared to be *the spec*.
  - The same day brought the Rust workspace, a hand-written lexer and the first tokens of Example 1.
- **2026-04-28: Phase 2, a vertical-slice game.** Building a real game in the new language produced a 15-item "frustration list", which drove Phase 3's design corrections: keyword arguments, `on update(dt)` inside states, and value-returning `and`/`or`.
- **2026-04-29: Phases 3–6 closed.** In three days Twe had all of these:
  - a bytecode VM;
  - `twec fmt`;
  - a tree-sitter grammar and a language server;
  - Hindley–Milner type inference with dimensional units (`5m + 3s` is an error);
  - cooperative fibers;
  - dialogue;
  - a first 3D renderer;
  - strict mode and a tutorial.

  427 tests.

### Act II: shipping features (2026-04-30 → 2026-06-02)

- **Phase 8.5 (closed 2026-05-01): NaN-tagged 64-bit values and a tracing garbage collector,** following *Crafting Interpreters* ch. 30. The honest result: the 3× VM speed-up target was **not** met, and the closeout says so.
- **Phase 9 (closed 2026-05-04): procedural visuals.** `visual` blocks compile to WGSL shaders; a procedural fire shader written in Twe renders end to end.
- **Phases 10–12 (closed 2026-05-04/05):**
  - UI widgets, a pause menu, settings, localization;
  - a crash reporter and profiler;
  - `twec build`, which produces a self-extracting Windows `.exe` that runs a game with no Twe install.
- **Phases 13–16 (2026-05-06):** modules, `twec verify` with machine-readable JSON diagnostics, `@deprecated`, and `survive_beta`, a 1,264-line Vampire-Survivors-style game built in Twe.
- **Phases 17–26 (2026-05-07):** the 3D commercial arc:
  - glTF models, textures, rapier physics;
  - point lights, shadow maps, GPU skinning;
  - HDR with ACES tonemapping, frustum culling.
- **Phases 27–41 (2026-05-09 → 05-11):**
  - **Genuinely implemented:** genre examples, bloom, cascaded shadows, fixed-timestep physics, replays, LAN lockstep netcode, open-world spatial structures.
  - **LLM tooling** (Phase 33): an MCP server, a GBNF grammar export for constrained decoding, fixes attached to `verify` diagnostics, a benchmark harness.
  - **Scaffolding only:** a second wave (rollback netcode, mobile, consoles, MMO, Workshop) shipped mostly *interfaces without working runtimes*. This mattered later (Act III).
- **v1.0.1 / v1.0.2 (closed 2026-05-18 / 05-26):** the polish releases: procedural VFX, tweens, 2D lighting, save migrations, localization plurals, `twec doctor`. 1,004 tests.
- **2026-06-01: the VM strategy decision.** The bytecode VM was measured to be *slower* than the tree-walker on the game-representative benchmark, so the tree-walker became the canonical runtime.

### Act III: the audit and the Web3D pivot (2026-09-27 → )

- **2026-09-27: an in-depth audit.** Stepping back produced uncomfortable findings:
  - **A memory-safety bug:** a garbage-collector use-after-free, reproducible by iterating a list comprehension while allocating.
  - **Dynamic scoping:** a `let` inside a function leaked into globals, and callees could see their caller's parameters.
  - **Scaffolding presented as features:** mobile, console, MMO and rollback were stubs.
  - **3D had been crashing:** `twec play3d` had crashed on its first frame for four months without any test noticing, because no test rendered anything.
- **The pivot** ([`changes/2026-09-27-web3d-pivot.md`](changes/2026-09-27-web3d-pivot.md)), set against a researched *Web3D Engine Architecture Expert Brief*:
  - **New target:** Twe becomes a **browser-first 3D language + engine**, one Rust + wgpu kernel running on WebGPU in Chrome and natively.
  - **New v1.0 goal:** a top-down 3D "survivors" game at 60 fps in Chrome.
  - **Plan:** seven milestones, M0–M6, with hard exit criteria; scaffolding moved behind `--features experimental`, and the README stopped claiming it.
- **M0, truth and safety (closed 2026-09-27):**
  - GC soundness, with a stress mode that poisons freed memory and a 1,000-seed differential fuzzer;
  - shipping-bug fixes;
  - the browser build compiling again.
- **M1, a correct and faster interpreter (closed 2026-09-28):**
  - **Lexical scoping** through a resolver pass, in the tradition of *Crafting Interpreters* ch. 11.
  - **The interpreter got 2.8–4× faster.** The entity-update benchmark went from 1,205 to 313–373 ns.
  - **The bytecode VM was deleted** (−8.5k lines), because keeping two runtimes meant every semantic change landed twice.
- **M2, the engine in the browser (closed 2026-09-28):**
  - The renderer was extracted into a kernel that knows nothing about Twe syntax, with a WebGPU browser shell and `twec build --target web`.
  - `hello_3d` runs in Chrome at **142–145 fps** (a 144 Hz display).
  - **Bugs found on the way:** Chrome's shader compiler rejected a shadow-sampling call that desktop drivers accepted (a black screen until fixed), and wgpu had to be upgraded from 22 to 30.
- **M3, entities at scale (in progress, started 2026-09-28):**
  - **The benchmark** is 5,000 enemies seeking the player (`examples/swarm_3d.twe`). The baseline in Chrome was **39–49 fps**.
  - **`look:` blocks** are a new language construct: an entity *declares* how it looks, and the engine draws it with no per-entity script. The frame rate jumped to **114–127 fps**.
  - **A browser CPU profile** showed 18% of each frame going to clock reads, a garbage-collector detail invisible in native benchmarks. Fixing it brought the swarm to **134–141 fps**.
  - **Interpreter work, by measurement.** The plan said to rebuild entity storage as columns (SoA), but measurement showed per-entity dispatch was only ~4% of the cost. So the work went where the profile pointed, in eleven measured steps:
    - names resolved to slots instead of looked up by string;
    - globals and fields read through cached positions;
    - tuples (every `vec3`) allocated once instead of three times.
  - **Result:** one enemy's update went from **2,459 ns to 668 ns** natively (3.7×). In wasm, the script tick for 5,000 enemies went from ~8 ms to a **4.3 ms median**, against a 4 ms goal.
  - **A HUD in 3D (2026-09-28).** Health bars and score text draw over the 3D scene, the last feature a survivors game needed from M3.
  - **Procedural surfaces reach the game (2026-09-28).** `material: Fire` paints an entity's mesh with a `visual` block, the code-only shader system from Phase 9, now compiled into the 3D renderer. The headline "procedural visuals from code" feature now works in the game itself, not only in a standalone viewer.
  - **`look:` learns to turn (2026-09-28).** `facing` rotates an entity on the GPU, shadows included, and `math.atan2` points it along its heading.
  - **M3 closed (2026-09-28)** with 5,000 enemies at 134–141 fps in Chrome, and new language surface: `look:` with mesh, tint, scale, facing and material. Missed honestly: the wasm tick sits at 4.3 ms against a 4 ms goal.
  - **Stopped deliberately, 7% short of the 4 ms wasm goal.** The frame-rate goal was already met with room to spare, and the next step (compiling to closures) would mean rewriting how suspended code resumes. That is kept in reserve until a real game needs it.

---

- **2026-09-28: the bar is raised to "better than Three.js."**
  - **The decision:** asked whether the plan covered graphics quality, the honest answer was no. Twe's renderer was behind Three.js on image quality. The maintainer set the new bar, "better and more advanced than Three.js", with no deadline.
  - **The plan:** a new milestone, M7, was written into the plan. Tier 1 is parity: physically based materials, environment lighting, anti-aliasing, AO, post-processing. Tier 2 goes beyond: GPU-driven rendering for 100k instances, hundreds of lights, a million GPU particles, and procedural materials written in Twe code.
  - **The test:** measured against Three.js with the same assets on the Khronos glTF sample scenes, with published image-error numbers. Where Twe loses, it will say so.

- **2026-09-28: M4 opens, and the game exists.** `survive3d`, the top-down 3D survivors game that defines v1.0, is playable: waves, a boss, three weapon types, XP and level-ups, pause, game over and restart. A headless test plays a whole run with scripted keys.
  - **It found four engine gaps, all fixed the same day:** menus couldn't draw in 3D; the browser only knew 11 keys; a documented math function didn't exist; and 3D colours had always been washed out, because colours were never converted from the sRGB values authors pick.

- **2026-09-28: M4 finished in code, the same day it opened.** `survive3d` became a real small game, and the engine grew what that took.
  - **In the browser:** sound, saved best runs, pausing when you switch tabs, mouse and gamepad, a clickable upgrade picker, and a loading screen with a progress bar.
  - **An animated hero:** a walking character built entirely from code, like the game's sound effects. No art tools were used.
  - **A net-ready spine:** every input now reaches the game as one command per simulation tick, so any run can be recorded and replayed exactly. Building it fixed a real bug: on 144 Hz screens, key presses could vanish or count twice.
  - **Soak-tested:** a scripted player ran ten 10-minute sessions natively and ten in WebAssembly, dying and restarting 3–6 times each, with no crash and no memory growth. The native and browser builds played the *same* sessions move for move: deterministic across machines, which replays and multiplayer depend on.
  - **Inside the editor:** TweEngine Studio now runs the game in a panel beside the code and reloads it on every save.
  - **Still to do by hand:** the itch.io upload. The 1 MB build is ready.

- **2026-09-29: M7 opens with the engine's plumbing.** Before adding any of the effects that should beat Three.js, the renderer was rebuilt around a *render graph*: each frame declares its passes and what they read and write, and the engine works out order, drops unused work, and shares GPU memory between passes. The proof it changed nothing: every test image came out byte-for-byte identical. The plan puts a measurement harness next, so every later graphics feature is scored against Three.js with numbers, not screenshots.

- **2026-09-29: the scoreboard.** Before improving a single pixel, Twe got an honest scoreboard. Twelve standard 3D test scenes from the Khronos group are rendered by Twe and by Three.js from identical files, and each image is scored against a Hollywood-grade path-traced reference with NVIDIA's ꟻLIP image metric. The starting score: **Twe 0.348, Three.js 0.186** (lower is better); Twe's error is about twice Three.js's. The table is committed, so every future improvement shows up as a number in the project history. Its first run already caught a real Twe bug: models with many materials were painted with just one.

- **2026-09-29: real materials.** Twe's renderer learned physically based materials: metal that looks like metal, bumpy surfaces from normal maps, glowing parts, textures that tile and rotate as the model's author intended, and a separate material for every part of a model (before, a whole model wore one texture). On the scoreboard Twe went from 0.348 to **0.295** (Three.js: 0.186), about a third of the gap closed, and on one test scene it now ties Three.js. What's left is mostly lighting from the environment, the next step.

- **2026-09-29: lit by the world around it.** Twe learned image-based lighting: a panoramic HDR photo of a room or a sky now lights the scene, with shiny surfaces reflecting it and rough ones glowing with its colours, the technique behind every modern game and product viewer. The scoreboard went from 0.295 to **0.198**; Three.js scores 0.186. The gap that started at 0.162 is now 0.012, and on one scene Twe scores better than Three.js. The scoreboard also caught a quiet bug: models without surface normals had been lit as if every face pointed at the ceiling.

- **2026-09-29: smooth edges, and an honest non-result.** Twe's 3D now renders with anti-aliasing (4× MSAA always, plus optional temporal AA that also calms shimmering highlights). The surprise was on the scoreboard: MSAA moved the score by essentially nothing, because at the benchmark's resolution edges are a sliver of the image. The notes say so plainly. Error maps showed the real remaining gap is in glossy reflections and missing material types, which set the agenda for the next sessions.

## Themes worth telling

- **The examples are the spec.** Features ship only when a real program needs them. `look:` exists because the 5,000-enemy benchmark showed per-entity drawing code was the single biggest browser cost.
- **Measure, then decide.** Several planned designs changed when measurements disagreed: the bytecode VM (deleted), columnar entity storage (deferred), and "the renderer must be the bottleneck" (it used 0.4 ms of a frame).
- **Honesty in the record.** Every milestone ends with a closeout note that lists what *didn't* ship. The Act III audit is part of the story, not hidden from it.
- **Built for humans and language models alike.**
  - An LL(1) grammar, exported as GBNF for constrained decoding.
  - Structured JSON diagnostics with machine-applicable fixes.
  - An MCP server.
  - A measured benchmark of LLM authoring (planned, web3d-M5) that includes a kill line: if it doesn't beat a Python baseline, stop claiming it as an advantage.
- **An AI pair-programmer on a real language project.** The work was done in sessions with Claude, with the human setting direction and making the design calls: pivot, SoA versus interpreter-first, scaffolding policy.
