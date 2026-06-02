# Twe — LLM Authoring Primer

> Canonical grounding for any model writing Twe. This is the single source of
> truth fed into the MCP `instructions` field, the `twe://guide` MCP resource,
> and Twe Studio's in-app AI prompt. If you are an AI assistant being asked to
> write or edit a `.twe` file, **read this first** — Twe is a custom language and
> your prior knowledge of Python/Lua/GDScript will mislead you on the details.

## What Twe is

Twe is a **game-first scripting language** (Rust-implemented runtime) where game
concepts are *language constructs*, not library calls. A 2D game, a 3D game, or a
shader effect is written directly in `.twe` files and run with `twec play`,
`twec play3d`, or `twec play_visual`. It is indentation-based (Python-family) but
is **not Python** — the rules below differ in ways that matter.

## The five principles (in priority order)

1. **Game concepts are first-class.** `entity`, `state`, `scene`, `dialogue`, `visual`, `particles` are keywords.
2. **One obvious way per concept.** Single inheritance, one method-call syntax, one OOP idiom.
3. **No silent footguns.** 0-indexed arrays; **only `false` is falsy** (`0`, `""`, `nil`, empty list are all truthy); dimensional units are enforced.
4. **AI-legible by design.** Small LL(1)-ish grammar, structured JSON diagnostics, round-trippable AST.
5. **Engine-native.** The runtime *is* the engine; game objects are first-class Twe values.

## Golden rules for generating Twe (read before you write)

- **Never invent stdlib functions.** Call `stdlib_lookup` / `stdlib_list` (or read the stdlib manifest) to confirm a name and its parameters exist. Twe has ~360 builtins across ~50 categories; guessing names is the #1 error.
- **Always `verify` before you claim done.** Run the `verify` tool on your output; if it returns errors, apply the structured `fix` patches and re-verify. Verified-clean source is the contract.
- **Drawing only inside `on render():`.** Calling `rect`/`circle`/`text`/`sprite` outside a render handler is a runtime error. Do state mutation in `every` / `on update(dt)`.
- **State transitions (`-> name`) are only legal inside a `state` block.** Code after a `->` is dead.
- **Keyword arguments must follow all positional args** and use `name: value` (e.g. `rect(at: (10,20), size: (100,50), color: color.red)`).
- **4-space indentation.** Don't mix tabs and spaces in one file (parse error).
- Prefer editing inside the block the user is focused on; keep changes minimal.

## The six core block keywords

```twe
scene Pong:            # a screen / game mode; holds vars + a state machine
entity Slime:          # a spawnable game object with fields, methods, lifecycle
state playing:         # a state-machine state (inside scene/entity/ai)
dialogue MeetMerchant: # sequenced say/choice script
visual Fire:           # compiles to a fragment shader (pixel(uv, time) -> color)
particles Sparks:      # an emitter; per-particle on_spawn(p) / on_update(p, dt)
```

`item`, `inventory`, `ai`, `tilemap`, `save` are stdlib-defined patterns that
desugar to `entity` + convention; the six above are the real keywords.

## Variables, types, operators

- `let x = ...` is **immutable**; `var x = ...` is **mutable**. Shadowing within a block is a compile error.
- Types are inferred (gradual typing). Optional annotations: `var hp: int = 100`. Optional types: `T?`. Unions: `A | B`.
- `and` / `or` **return one of their operands** (not a strict bool) and short-circuit. Because only `false` is falsy, `count or default` returns `count` even when `count == 0`. `not` returns a strict bool.
- `==`, `!=`, `<`, `<=`, `>`, `>=`; arithmetic `+ - * / %`. **`%` is the percent-literal suffix** (`5%`), not modulo — use `math.mod(a, b)` for modulo.
- Assignment ops: `=`, `+=`, `-=`, `*=`, `/=`.

### Literals you won't find in other languages

- **Tuples / vectors:** `(1, 2)`, `(x, y, z)`. Tuple arithmetic works: `(x,y) + dir`. Access `.x`, `.y`.
- **Ranges:** `10..15` (inclusive), `0..<count` (exclusive end).
- **Percent:** `5%` (type `percent`; not `0.05`).
- **Units (dimensional, enforced in strict/verify):** durations `0.5s 200ms 5min`; lengths `5m 100px 30cm`; angles `90deg`; compound `5 m/s`. `5m + 3s` is a type error.
- **String interpolation:** `"Score: {score}"` — `{expr}` is inlined.
- **Lists:** `[1, 2, 3]`; comprehensions `[x * 2 for x in 0..5 if x > 1]`.

## Events, frames, time

```twe
on update(dt):          # every frame; dt is fixed at 1/60s for determinism
on render():            # every frame; ONLY place drawing is allowed
on key_press.space:     # edge-triggered (one frame on press)
on hp < 20%:            # predicate event — fires on false→true transition
on Slime.death(s):      # named event when an instance is despawned
every 150ms:            # timed clock (inside a state); fires on each interval
```

- Input: `key.w` (held) vs `key_press.w` (one frame). `mouse.x`, `mouse_press.left`, `mouse_held.left`. `gamepad.a`, `gamepad_axis.lx`.
- `time.dt` = seconds since last tick; `time.physics_dt` = constant 1/60s.

## State machines

```twe
scene Game:
    var score = 0
    initial: playing          # required: which state starts active

    state playing:
        on key_press.space:
            -> paused          # immediate transition; only valid inside a state
        every 1s:
            score += 1
        on render():
            text("Score: {score}", at: (10, 10), size: 20, color: color.white)

    state paused:
        on enter:              # runs when state becomes active
            sound.play(chime)
        on exit:               # runs when leaving (synchronous; no wait, no ->)
            ...
        on key_press.space:
            -> playing
```

Exactly one state is active. `on ...` handlers and `every` clocks inside a state
are active only while in that state. `wait <duration>` suspends inside a state's
on-entry body without leaving the state.

## Entities

```twe
entity Slime extends Enemy:        # single inheritance via `extends`
    var hp: int = 30
    var speed = 40.0
    function hurt(amount: int):     # methods receive `self` implicitly
        self.hp -= amount
    on update(dt):                  # runs for the lifetime of each instance
        self.pos = self.pos + (self.speed * dt, 0)

# elsewhere:
spawn Slime at (100.0, 200.0)
for s in entities.of(Slime):
    s.hurt(1)
let n = entities.count(Slime)
```

## Stdlib map (call `stdlib_lookup` for exact signatures)

`math.*` (sqrt, floor, clamp, sin, noise, mix, pi…) · `random.*` (float, int(0..<n), choice, shuffle) ·
`color.*` (named: `color.red/green/white/…`; `color.hsv`, `color.from_hex`) ·
**draw (render-only):** `rect`, `rect_outline`, `circle`, `circle_outline`, `line`, `text`, `sprite`, `sprite_frame` ·
input: `key/key_press`, `mouse/mouse_press/mouse_held`, `gamepad*` ·
`camera.*` / `camera2d.*` · `fx.*` (hit_flash, screen_shake, damage_number…) · `tween.*` (ease, lerp) ·
`light2d.*` · `sound.*` / `music.*` · `physics.*` / `physics2d.*` · 3D: `light/sun/postfx/mesh/world/terrain` ·
`save.*` / `settings.*` · `lang.*` (i18n) · `net/rollback/mmo` (multiplayer) · `ui.*` widgets (`button`, `slider`, `progress_bar`…) · `tilemap*`.

## A complete, verified example (Snake)

```twe
scene Snake:
    var snake = [(10, 7), (9, 7), (8, 7)]
    var dir = (1, 0)
    var next_dir = (1, 0)
    var food = (15, 7)
    var score = 0

    initial: playing

    state playing:
        on key_press.right:
            if dir != (-1, 0):
                next_dir = (1, 0)
        on key_press.left:
            if dir != (1, 0):
                next_dir = (-1, 0)
        on key_press.up:
            if dir != (0, 1):
                next_dir = (0, -1)
        on key_press.down:
            if dir != (0, -1):
                next_dir = (0, 1)

        every 150ms:
            dir = next_dir
            let new_head = snake[0] + dir
            if new_head.x < 0 or new_head.x >= 20:
                -> game_over
            if new_head.y < 0 or new_head.y >= 15:
                -> game_over
            if new_head in snake:
                -> game_over
            if new_head == food:
                snake.prepend(new_head)
                score += 1
                food = (random.int(0..<20), random.int(0..<15))
            else:
                snake.prepend(new_head)
                snake.pop_back()

        on render():
            for cell in snake:
                rect(at: (cell.x * 24, cell.y * 24), size: (24, 24), color: color.green)
            rect(at: (food.x * 24, food.y * 24), size: (24, 24), color: color.red)
            text("Score: {score}", at: (10, 20), size: 20, color: color.white)

    state game_over:
        on render():
            text("Game Over", at: (200, 200), size: 32, color: color.red)
        on key_press.r:
            snake = [(10, 7), (9, 7), (8, 7)]
            dir = (1, 0)
            next_dir = (1, 0)
            food = (15, 7)
            score = 0
            -> playing
```

## Tooling contract (MCP tools available to you)

- `stdlib_list(category?)` / `stdlib_lookup(name)` — confirm callable surface. Use before writing calls.
- `grammar(format?)` — the exact grammar (`gbnf` | `json-schema` | `ebnf`). Resource: `twe://grammar`.
- `parse(source)` — AST or a `{line, col, message}` syntax error.
- `verify(source)` — JSON v2 diagnostics with machine-applicable `fix` patches. **Run this and fix until clean.**
- `format(source)` — canonical pretty-print.
- `apply_patch(source, edits)` — apply structured `{line, col, len, replace}` edits (consumes a verify `fix`).

The full guide is also available as the `twe://guide` resource; example programs as `twe://examples/<name>`.
