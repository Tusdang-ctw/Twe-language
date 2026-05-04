# 2026-05-04 — Phase 8 closeout

## Status: closeout note. Closes Phase 8 (v0.2 — Foundations for shipping). Phase 8.5 (NaN tagging + tracing GC) closed separately on 2026-05-01 (`docs/changes/2026-05-01-phase-8.5-closeout.md`); this note draws the line under the Phase 8 surface itself.

## Background

Phase 8 opened on 2026-04-29 immediately after Phase 6 closed and ran in parallel with Phase 7's release-engineering line item. Theme per `docs/05-roadmap.md` §"Phase 8": *close the load-bearing gaps that block any real game from running at all.* Not polish — absence-of-features that make a Survivors-class game impossible.

Seven feature sessions shipped between 2026-04-29 and 2026-04-30, each with its own session note:

| # | Surface | Session note |
|---|---------|--------------|
| 1 | `.glb` mesh import | `2026-04-29-v0.2-session-1-glb-import.md` |
| 2a | Resumable `if` / `while` blocks (tree-walker) | `2026-04-29-v0.2-session-2a-resumable-blocks.md` |
| 2b | Function-body `wait` on tree-walker | `2026-04-29-v0.2-session-2b-function-body-wait.md` |
| 2c | VM nested-block `wait` parity | `2026-04-30-v0.2-session-2c-vm-wait-parity.md` |
| 3 | Mouse input (both backends) | `2026-04-30-v0.2-session-3-mouse-input.md` |
| 4 | Save / load bottom layer | `2026-04-30-v0.2-session-4-save-load-bottom.md` |
| 5 | Audio v2 (volume + music + stop) | `2026-04-30-v0.2-session-5-audio-v2.md` |
| 6 | Tilemap (stdlib-builtin form) | `2026-04-30-v0.2-session-6-tilemap.md` |
| 7 | VM function-body `wait` (multi-frame fiber save) | `2026-04-30-v0.2-session-7-vm-function-body-wait.md` |

The remaining Phase 8 line item — NaN-tagged 64-bit values + incremental tracing GC — broke out as Phase 8.5 per `docs/08-nan-tagging.md` because the migration is genuinely 9 careful sub-sessions and rolling it into a Phase 8 close-out would either skip GC entirely or break the migration. Phase 8.5 closed 2026-05-01.

## Phase 8 — what shipped

Against the roadmap §"Phase 8 — Components" list:

- ✅ **Tilemap rendering + collision** (Example 9, stdlib form). `tilemap(layout, tile_size, tiles)` + `tilemap_render` + `tilemap_at` + `tilemap_solid_at` stdlib builtins. The `tilemap Dungeon:` block syntax is a v0.3+ follow-on parser session (captured in `docs/01-examples.md` "Runtime delivery status").
- ✅ **Save / load** (Example 7, bottom layer). `save_to(path, value)` / `load_from(path)` stdlib builtins built against the `docs/07-save-system.md` schema. The `save SaveSlot:` block syntax + version-migration syntax is a v0.3+ follow-on (still listed under "What is open" in `CLAUDE.md`).
- ✅ **Mouse input.** `mouse.x`, `mouse.y`, `mouse_press.<button>`, `mouse_held.<button>` ship on both backends.
- ✅ **Function-body `wait` on the bytecode VM.** Multi-frame `Vec<BcFiberFrame>` save (the deferred half of session 2c). Tree-walker received it earlier via session 2b; the VM mirror landed in session 7.
- ✅ **Audio v2.** `sound.play(handle, volume:, pitch:)`, `music.play(handle, loop:)`, mixer channels, fade-in / fade-out.
- ✅ **`.glb` mesh import.** First v0.2-line carry-over from Phase 5's deferral list.
- ➡️ **NaN-tagged 64-bit values + incremental tracing GC** — broke out as Phase 8.5; closed 2026-05-01.

## Phase 8 exit criteria

Per the roadmap §"Phase 8 — Exit criteria":

- [x] **Example 7 (save/load, layer 1) and Example 9 (tilemap, stdlib form) run on both backends.** Met as of session 6 (tilemap) and session 4 (save).
- [x] **Function-body `wait` works on both backends.** Met as of session 7.
- [x] **Mouse + audio v2 surfaces shipped.** Met as of sessions 3 + 5.

The *runtime perf* criteria (≥3× tree-walker speedup, no visible GC pauses on a 1k-entity 60fps test) are inherited by Phase 8.5 and tracked in its own closeout. Phase 8.5 met the no-GC-pause criterion via the aggressive-GC stress tests; the 3× speedup criterion is **not met** and lives on as a follow-on perf phase.

## Deferred (to v0.3 or later)

- **`tilemap Dungeon:` block syntax** — language-level form to wrap the stdlib builtins. v0.3+ parser session.
- **`save SaveSlot:` block syntax + version migration** — language-level form on top of v0.2's stdlib bottom layer. v0.3+ parser/codegen session.
- **3× bytecode-VM speedup vs pre-tag baseline** — Phase 8.5 perf gap. Captured in `docs/changes/2026-05-01-phase-8.5-closeout.md` §"What slipped" with the follow-on agenda (criterion harness, profile-guided tuning, dispatch-loop redesign).

## Verification

- `cargo build --release` — clean.
- `cargo clippy -- -D warnings` — clean.
- `cargo test` — **502 tests pass** at Phase 8.5 close; **544 tests pass** as of this note (gain came from Phase 9's 11 sessions running in the same calendar week — they're attributed to Phase 9, not Phase 8).
- All v0.2-feature programs in `examples/` (`hello_glb.twe`, `mouse_demo.twe`, `save_demo.twe`, `audio_demo.twe`, `tilemap_demo.twe`, `wait_in_function.twe`, `wait_nested_blocks.twe`) run on the relevant backend.

## Doc edits applied as a result

- `CLAUDE.md` Phase discipline updated: Phase 8 closed; Phase 8.5 closed (already noted at its own closeout); Phase 9 listed as the next-up phase.
- `docs/05-roadmap.md` Phase 8 §"Status" rewritten from "substantively complete" to "closed 2026-05-04 per `docs/changes/2026-05-04-phase-8-closeout.md`."
- `README.md` Status section: the v0.2 surface bullets ([x] mouse / save / audio / tilemap / `.glb`) collapse under a single "Phase 8 closed" pointer; NaN-tagging line moves to "[x] Phase 8.5 closed."