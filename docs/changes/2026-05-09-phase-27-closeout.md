# Phase 27 closeout — 2D genre reference examples

**Status:** codebase-closed 2026-05-09. All five sessions shipped.

The first phase of the post-v1.0 plan from `docs/05-roadmap.md` "Post-v1.0 — Phases 27–32." Three new example games pressure-tested the v1.0 stdlib in genres `examples/survive_beta/` didn't reach; one stdlib gap-closure pass landed the helpers the examples surfaced; one closeout (this note).

## Sessions shipped

| # | Surface | File |
|---|---------|------|
| 1 | `examples/platformer.twe` — coyote time, jump buffer, variable jump, AABB tile collision, one-way platforms | 245 lines |
| 2 | `examples/tetris.twe` — 7-bag, simplified SRS, line clears, DAS/ARR, ghost piece, level cadence | 484 lines |
| 3 | `examples/cards.twe` — Solitaire-Lite, drag-and-drop, layered z-order, snap-back, valid-target hint | 365 lines |
| 4 | Stdlib gap closure — `math.mod`, `random.shuffle`, `tilemap_solid_aabb`, `tilemap_aabb_touches` | `src/stdlib.rs` |
| 5 | This closeout note | `docs/changes/2026-05-09-phase-27-closeout.md` |

## Exit criteria

Per the Phase 27 entry in `docs/05-roadmap.md`:

- ✅ Each example ≤ 500 lines. Platformer 245, Tetris 484, Cards 365.
- ✅ No new stdlib functions added without a Principle 1 / Principle 3 justification. Each of the four shipped helpers absorbed a multi-call pattern that ≥ 2 examples were repeating, or fixed an outright friction (`%` is a percent suffix, not a binary modulo operator).
- ✅ README examples gallery updated. `platformer.twe`, `tetris.twe`, `cards.twe`, `flappy.twe` now appear.

## Stdlib delta

Four new builtins shipped in session 4:

| Function | Signature | Impl |
|----------|-----------|------|
| `math.mod` | `(a, b) -> int \| float` | Euclidean remainder. Errors on divisor 0. Negative `a` wraps to `[0, b)` so `math.mod(-1, 4) == 3`. |
| `random.shuffle` | `(list) -> nil` | In-place Fisher-Yates. Empty / single-element lists are no-ops. Determinism preserved across seed re-set. |
| `tilemap_solid_aabb` | `(map, x, y, w, h) -> bool` | True if any of the AABB's four corners falls in a tile carrying the `solid` trait. |
| `tilemap_aabb_touches` | `(map, x, y, w, h, name) -> bool` | True if any of the AABB's four corners falls in a tile of the given name. |

`docs/06-design-document.md` §7.2 / §7.3 / new §7.9b updated; `tests/programs/{math_mod,random_shuffle,tilemap_aabb}.twe` ship as the canonical reference programs; five new entries in `tests/eval.rs` pin the behavior.

The shared `tile_has_trait(map, x, y, trait_name)` helper in `src/stdlib.rs` is private — added to back the AABB queries without re-borrowing the map four times. Could absorb the `tilemap_solid_at` body too; left untouched per the "don't refactor beyond the task" guidance in CLAUDE.md.

## Friction punch list — closed vs. deferred

The three new examples surfaced eight inline `GAP-N` markers. Session 4 closed four; the rest are honestly deferred.

**Closed in session 4:**

1. ✅ **`math.mod(a, b)`** — Tetris GAP-4 (caught at parse time). Twe's `%` is the percent-literal suffix; no Twe example before this had needed binary modulo, and the open-coded "if x < 0: x += n; if x >= n: x -= n" idiom was clearly worse than a builtin.
2. ✅ **`random.shuffle(list)`** — Tetris GAP-3, Cards (silent). Three examples were repeating ~10 lines of Fisher-Yates each.
3. ✅ **`tilemap_solid_aabb(map, x, y, w, h)`** — Platformer GAP-1.
4. ✅ **`tilemap_aabb_touches(map, x, y, w, h, name)`** — Platformer GAP-1 (paired).

**Deferred (with reasons):**

5. **`tilemap_sweep(map, x, y, w, h, dx, dy)`** — Platformer GAP-2/3 + Cards GAP-1 mention. Real swept-AABB primitive. Deferred because (a) only one example has the underlying tunneling risk and `max_fall_v = 900` masks it in practice; (b) a correct implementation needs proper line-vs-tile collision math and a hit-normal return shape; (c) one-way-platform `prev_bottom` plumbing in `examples/platformer.twe` would also depend on it. Re-entry: a Phase 28+ session whenever a second example pressures the high-velocity case (e.g., a fast-faller boss room, or the bullet-hell follow-on after Phase 31).
6. **`key_repeat(name, das, arr)`** — Tetris GAP-2. ~30 lines per direction for arcade key-repeat is a real ergonomics gap. Deferred because the stateful timer registry is non-trivial (per-key timer state survives across frames; needs lifecycle for keys never released between scenes), and the userland idiom works. Re-entry: revisit if a second example pressures it (a fighting-game Phase 30 example, or the rhythm-game Phase 29 demo under "Determinism layer").
7. **`mouse_release.<button>` ambient field + `mouse_released(name)` builtin** — Cards GAP-2. Synthesizing release via `was_held && not mouse_held.left` works. Deferred because the runtime extension crosses three files (`stdlib.rs` ambient registration, `play.rs` per-frame state setter, `eval.rs` field-access dispatch), and the userland workaround is one variable + one line. Re-entry: bundle with the larger "input layer rewrite" if Phase 29 (determinism) needs sample-accurate input edges.
8. **`hit_box(at, size)` / `point_in(box, point)`** — Cards GAP-3. The userland `point_in(px, py, x, y, w, h)` is a 5-line helper. A stdlib version doesn't materially beat it, and Principle 2 ("one obvious way per concept") cuts against shipping a one-line wrapper. Re-entry: possibly rolled into a Phase 30 (web target) UI helper layer once a fourth example needs it.

## Doc updates

- `docs/06-design-document.md` §7.2 (math) — added `math.mod`.
- `docs/06-design-document.md` §7.3 (random) — added `random.shuffle`.
- `docs/06-design-document.md` §7.9b (tilemaps) — new subsection consolidating `tilemap`, `tilemap_render`, `tilemap_at`, `tilemap_solid_at`, `tilemap_solid_aabb`, `tilemap_aabb_touches`.
- `examples/platformer.twe` — switched to the AABB builtins; the inline `aabb_solid` / `aabb_touches` user functions deleted; `aabb_one_way` retained (still needs `prev_bottom`).
- `examples/tetris.twe` — `refill_bag` shrank from ~13 lines to 5 via `random.shuffle`; `try_rotate` swapped its hand-rolled wrap for `math.mod`.
- `examples/cards.twe` — `reset_game` deal shrank by ~9 lines via `random.shuffle`.
- `README.md` examples gallery — already updated in the session 1–3 commit. No edit this commit.

## Test delta

Five new tests in `tests/eval.rs`:

- `runs_math_mod` — pins the Euclidean remainder behavior on int / int + float / float + the rotation-wrap pattern from `examples/tetris.twe`.
- `math_mod_by_zero_errors` — pins the divisor-0 error message.
- `runs_random_shuffle` — pins determinism (re-seeded permutation matches), permutation preservation (no element added or dropped), and the empty / single-element no-op edges.
- `random_shuffle_on_non_list_errors` — pins the type-error message.
- `runs_tilemap_aabb` — pins `tilemap_solid_aabb` + `tilemap_aabb_touches` against a 5×3 fixture with sky / wall / spike tiles.

`cargo test --release` reports **742 passing** (was 737 at v1.0 codebase-close on 2026-05-06; +5 from this phase). `cargo clippy --release --all-targets -- -D warnings` clean.

## Visual playtest status

Not run from this terminal — none of `twec play examples/platformer.twe`, `twec play examples/tetris.twe`, `twec play examples/cards.twe` was opened in a window during the work. The user runs each interactively to confirm feel; tuning constants (`platformer.twe:46-52`, `tetris.twe:46-50`) are the dials. Visual playtest is the remaining manual step before treating Phase 27 as "shipped" rather than "codebase-closed."

## What this enables

- The **2D genre coverage** claim for the language now ships a reference implementation per genre family: arcade (`pong`, `flappy`, `snake`), bullet-hell (`survive_beta`), platform (`platformer`), grid puzzle (`tetris`), card / drag-UI (`cards`).
- The four new stdlib helpers absorb roughly 80 lines of repetitive bookkeeping across the three new examples and the existing `tilemap_demo.twe`. Future game examples in the same shape are correspondingly shorter.
- The **deferred** punch list (sweep / key_repeat / mouse_release / hit_box) becomes the entry pressure for Phase 28+ — when the next example forces one of those, that example's commit can fold the closure in.

## What does not change

- No grammar change. No new keyword. No type-system change. The four new builtins fit the existing `name(arg)` call shape.
- No regression on the v1.0 surface. All previous examples continue to parse + type-check + run.
- Phase 28 (3D commercial polish) entry is unblocked. The next session opens against [phase-17-closeout.md](2026-05-07-phase-17-closeout.md) deferrals (mipmaps + anisotropic) and [phase-24-26-closeout.md](2026-05-07-phase-24-26-closeout.md) deferrals (bloom, DoF, cascaded shadows, async preload).
