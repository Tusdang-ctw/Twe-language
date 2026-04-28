# 2026-04-28 — Phase 2 frustration list

> Per `docs/05-roadmap.md` Phase 2 exit criteria: "**The implementer
> has a list of language frustrations encountered during the build.
> This list is the input for Phase 3.**"

This doc captures every place where building Snake (`examples/snake.twe`),
Hero (`examples/hero.twe`), Survive (`examples/survive.twe`), and the
test programs in `tests/programs/` made the language design feel
awkward. Each item names the symptom, the workaround used, and the
shape of the eventual fix.

The frustrations sort roughly into three buckets: **runtime model
gaps** (things the tree-walker can't do that a real game wants),
**ergonomics** (things that work but are noisy), and **doc-spec drift**
(places where the design docs and the implementation now disagree).

## Runtime-model gaps

### F1. No keyword arguments — **CLOSED 2026-04-28 (Phase 3 session 1)**

Snake's `rect(at: cell * cell_size, size: ..., color: color.green)`
shape is in `docs/example-11-snake.md` NP5 and Examples 6, 7, 10.
The parser accepts only positional args. Every drawing call in
`examples/survive.twe` and `examples/snake.twe` uses positional form;
`text("Score: {score}", (10, 20), 18, color.white)` reads worse than
`text("Score: {score}", at: (10, 20), size: 18, color: color.white)`.

**Fix shipped:** parser now accepts `name: value` in arg lists
(positional-after-keyword is a parse error, same as Python).
`Value::Builtin` carries a `params: &'static [&'static str]` slice;
empty means variadic (kwargs rejected). A single `bind_kwargs`
helper distributes kwargs into a positional Vec for both Twe
functions/methods (uses `def.params`) and builtins (uses the
declared slice). Errors are explicit: unknown name, duplicate
binding, missing required param, kwargs on a variadic builtin.
All four drawing primitives plus survive/snake/hero/sprite_demo/
particles_demo are migrated to keyword-arg form. F15 closes with
the same fix.

### F2. Entities can't read scene fields without globals

`examples/survive.twe` has `var player_x = 320.0` at the top-level
specifically so `entity Monster`'s `update(dt)` can read the player
position. The scene-and-entities-as-coexisting-instances model has
no scoping rule for "the entity wants to query the active scene".
Workarounds in Phase 2: dump the relevant fields into globals.

**Fix shape:** an `engine` ambient (or a `scene` ambient) that
returns the current active scene Instance, so entity code can
`engine.scene.player_pos` (or similar). Phase 4 may dovetail this
with the Bevy-style query pattern from `docs/03-runtime.md`.

### F3. No collision query — **CLOSED 2026-04-28 (Phase 2 follow-on)**

Survive has bullets and monsters; checking which bullets are inside
which monsters means iterating `env.active_entities` from Twe code,
which we have no API for. The current Survive demo cheats: monsters
self-destruct only when they touch the player, no bullet/monster
collision. The parsed-but-dead `kills` counter is the visible
symptom.

**Fix shipped:** `entities.of(Class)` returns a list of live
instances; `entities.count(Class)` returns the count. Picked the
function-call form over the `Class.all` field-access form because it
needs no new grammar rule (Principle 4: predictable LL(1)-ish
grammar). Survive's Bullet now iterates `entities.of(Monster)` in
its `update(dt)` and despawns both on overlap. F6 closes with the
same fix.

### F4. No catch-up on `every` clocks under big dt — **CLOSED 2026-04-28 (Phase 3 session 2)**

`tick_scene` fires each every-clock at most once per frame. Real
macroquad frames at 60fps run dt ≈ 16ms, so a 16ms every-clock
fires exactly once. But `tick_frame(env, 0.5)` in the test harness
fires only once even though 31 ticks would be deserved — Survive's
spawn-timer falls progressively behind as dt grows. The test for
Survive had to use 120 small frames instead of 3 big ones.

**Fix shipped:** bounded catch-up. `tick_scene` now wraps each
clock's fire check in a `while` loop bounded by
`MAX_CATCHUP_FIRES_PER_FRAME = 8`. When the cap is hit, the timer
accumulator is reset to zero so the backlog doesn't compound on
the next frame. Eight 16ms ticks ≈ 128ms — enough to absorb a
slow first frame or a brief stall, while still bounded so a long
pause (debugger / alt-tab) can't lock the runtime in catch-up
forever. Tested via `tests/programs/catchup.twe` (5 deserved fires
per frame, all 5 happen) and `tests/programs/catchup_capped.twe`
(20 deserved per frame, exactly 8 happen).

### F5. No keyword distinguishing per-state on update

State bodies have `every <duration>:` and `on render():` but no
state-scoped `on update(dt):`. The top-level `on update(dt):` is
global to the program, not the active state. Survive worked around
by putting all per-frame logic into a single `every 16ms:` clock,
but that loses the dt parameter (the body uses `let dt = 0.016`,
a literal).

**Fix shape:** allow `on update(dt):` inside state bodies, dispatched
each frame with the real dt. Or fold `every <duration>:` and
`on update:` into a single concept where the duration of `on update`
is "every frame".

### F6. No direct way to query active-entities count or filter — **CLOSED 2026-04-28**

For collision, scoring, AI lookup ("nearest monster"), etc. — all
of these want list views of entities. Phase 2 punted by spawning
nothing-vs-nothing.

**Fix shipped:** see F3 — `entities.of(Class)` and
`entities.count(Class)`.

### F7. Strings are immutable; tuples too

The Snake game stores the snake body as `var snake = [(10, 7), ...]`
— a list of tuples. Mutating individual cells means re-creating
tuples (`pos = (pos.x + dx, pos.y + dy)`). Worked around with tuple
arithmetic helpers (`pos + dir`), but the syntactic noise inside
update bodies is still real.

**Fix shape:** considered too small to break tuple immutability;
look at this in Phase 4 alongside refinement types.

## Ergonomics

### F8. `let dt = 0.016` workaround for every-clocks — **CLOSED 2026-04-28**

(See F5.) Every-clocks have no implicit dt; the only available dt
is in top-level `on update(dt):`. Survive shoves `let dt = 0.016`
into the every body and hopes the real frame matches.

**Fix shipped:** `time.dt` ambient module-field is rewritten by
`eval::tick_frame` at the top of every frame. `every` clocks (and
any other code) now read `time.dt` instead of guessing. Note: this
is the *frame* dt, not the elapsed-time-since-last-fire of the
clock — that finer-grained accounting still needs F5.

### F9. Nested `if` chains for direction handling

```
on key_press.right:
    if dir != (-1, 0):
        next_dir = (1, 0)
on key_press.left:
    ...
```

Snake repeats this four times. A guarded transition syntax — `on
key_press.right when dir != (-1, 0): next_dir = (1, 0)` — is
suggestive but not in any example, so deferring.

### F10. No early-return / labelled break in for loops

`for cell in snake:` followed by `if cell == new_head: ...`
collision detection wants to break-and-flag. Currently a flag
variable is needed, set inside the loop, checked after.

**Fix shape:** loop labels (`break outer`) like Rust. Phase 3 if
several Phase 2 games want it.

### F11. `or` returns the value, not a strict bool

`if key.right or key.d: ...` works because of truthiness, but
`a or b` returning `a` (when `a` is truthy) means downstream code
can't rely on the result being a Bool. Survive happens to only
chain `or`s inside `if` conditions, so this hasn't bitten yet.

**Fix shape:** consider whether `or` should always return Bool
(strict) or stay value-returning (Python-like). Locked decision for
v0.2; recorded in `docs/changes/`.

### F12. `every <duration>:` ergonomics with `..ms` and `..s`

`every 16ms:` and `every 150ms:` work; `every 1s:` works.
Conversions between them are runtime, not type-level — `every
0.0166s:` is the same as `every 16.6ms:` but the user had to know
the conversion ratio. Phase 4 type-system work should make these
interchangeable at the type level (per `docs/02-type-system.md
§5.5`).

## Doc-spec drift

### F13. `docs/06-design-document.md §10.1` keyword list omits
many keywords the implementation now has

The spec's keyword table doesn't list: `spawn`, `despawn`,
`elif`, `else`, `var`. All five are required by examples and the
implementation lexes all five as keywords. The spec table also
omits `function` (used in declarative-block method declarations)
and `and` / `or` / `not` (it has `not` only).

**Fix shape:** rewrite §10.1 as a definitive list pulled from
`src/lexer.rs`. Either generate it programmatically in the build
or schedule a one-time doc audit.

### F14. `docs/02-type-system.md §5.2.1` and `docs/01-examples.md`
disagree on the loader API

Doc 02 shows `hero = sprite.load("hero.png")`; doc 01 (post the
2026-04-27 sprite reconciliation) shows `let hero = load("hero.png")`.
Both work in v0.1. Already noted in `notes/future-phases.md`.

### F15. `docs/example-11-snake.md` uses keyword args throughout — **CLOSED 2026-04-28**

The Snake spec example uses `rect(at: ..., size: ..., color: ...)`
and `text(..., at: ..., color: ..., size: ..., align: ...)`.
The Phase-2 implementation has no keyword args (F1). The shipping
`examples/snake.twe` uses positional form and is therefore not
byte-identical to the design doc's reference code.

**Fix shipped:** `examples/snake.twe` now uses keyword-arg form
for every drawing call, matching `docs/example-11-snake.md`. The
`align:` parameter on `text()` is still not supported (text is
top-left only in v0.1) — that's a separate stdlib gap, not an F15
problem.

## What survived contact unchanged

The five principles (game concepts first-class, one obvious way,
no silent footguns, AI-legible grammar, engine-native) all held.
Tree-walker performance is fine for ~50 entities at 60fps. Hot
reload via mtime polling works without surprises. Snake at 70 lines
and Survive at ~110 lines are well under the roadmap's 500-line
budget. The state-machine semantics held up cleanly across Snake
and Survive.

The pitfalls list in `docs/03-runtime.md` was not invalidated by
anything in Phase 2.

## Total: 15 frustrations

Three categories, none of them deal-breakers. The roadmap planned
this list as Phase 3's input. A reasonable Phase 3 ordering, by
my read of the items above:

1. **Bytecode VM** — the language slice is now stable enough to
   compile rather than tree-walk. Per the roadmap, Phase 3 starts
   here.
2. **F1 keyword arguments** — small parser change, big readability
   win, unlocks F15's example-11 alignment.
3. **F2 / F3 / F6 entity queries** — group together as the
   "iteration on dynamic instances" theme.
4. **F4 catch-up** — once we've measured frame-time variance in
   real games.
5. **F5 / F8 state-scoped on update** — design call; folds into
   the every / on-update unification question.

The rest can stay deferred until further pressure.
