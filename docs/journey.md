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
  - **Stopped deliberately, 7% short of the 4 ms wasm goal.** The frame-rate goal was already met with room to spare, and the next step (compiling to closures) would mean rewriting how suspended code resumes. That is kept in reserve until a real game needs it.

---

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
