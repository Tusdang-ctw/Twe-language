# Example 11 — Snake (the grid-game pressure test)

> A standalone addition to `01-examples.md`. Snake is structurally different from the original ten examples: it's grid-based and tick-driven rather than continuous, which exposes design pressures the original ten missed.
>
> The point of this example is not "Twe can build Snake" — of course it can. The point is **to find gaps in the language design before any code is written.** Snake is unusually good for this because every Snake implementation needs: a discrete tick clock, a grid coordinate space, mutable lists with prepend/pop, set membership testing, simple shape drawing, UI text, and a restart flow. None of those were forced by the original ten.

---

## The Twe code

```twe
scene Snake:
    grid_size: (20, 15)
    cell_size: 24px
    tick_rate: 150ms

    var snake: list of (int, int) = [(10, 7), (9, 7), (8, 7)]   # head first
    var direction = (1, 0)
    var next_direction = (1, 0)     # buffered to prevent reversing into self
    var food = (15, 7)
    var score = 0

    initial: playing

    function random_free_cell() -> (int, int):
        var free: list of (int, int) = []
        for x in 0..<grid_size.x:
            for y in 0..<grid_size.y:
                let cell = (x, y)
                if cell not in snake and cell != food:
                    free.append(cell)
        return random.choice(free)

    function reset():
        snake = [(10, 7), (9, 7), (8, 7)]
        direction = (1, 0)
        next_direction = (1, 0)
        food = random_free_cell()
        score = 0

    state playing:
        on key_press.right: if direction != (-1, 0): next_direction = (1, 0)
        on key_press.left:  if direction != (1, 0):  next_direction = (-1, 0)
        on key_press.up:    if direction != (0, 1):  next_direction = (0, -1)
        on key_press.down:  if direction != (0, -1): next_direction = (0, 1)

        every tick_rate:
            direction = next_direction
            let new_head = snake[0] + direction

            # wall collision
            if new_head.x < 0 or new_head.x >= grid_size.x: -> game_over
            if new_head.y < 0 or new_head.y >= grid_size.y: -> game_over

            # self collision
            if new_head in snake: -> game_over

            # eat food?
            if new_head == food:
                snake.prepend(new_head)        # grow — keep the tail
                score += 1
                food = random_free_cell()
            else:
                snake.prepend(new_head)
                snake.pop_back()

        on render():
            for cell in snake:
                rect(
                    at: cell * cell_size,
                    size: (cell_size, cell_size),
                    color: color.green
                )
            rect(
                at: food * cell_size,
                size: (cell_size, cell_size),
                color: color.red
            )
            text("Score: {score}", at: (10px, 10px), color: color.white)

    state game_over:
        on render():
            text("Game Over",
                at: screen.center,
                color: color.red,
                size: 32px,
                align: center)
            text("Final score: {score}",
                at: screen.center + (0px, 40px),
                color: color.white,
                align: center)
            text("Press R to restart",
                at: screen.center + (0px, 80px),
                color: color.gray,
                align: center)

        on key_press.r:
            reset()
            -> playing
```

---

## What this demonstrates

The whole game is one scene with two states. The total program is under 70 lines. Every primitive used is implied either by the original ten examples or by a small, defensible extension. The implementation is roughly 25% shorter than the equivalent in Lua + Love2D, primarily because of `state`, `every`, and the structural sugar of declarative blocks.

---

## Implied decisions (recap of existing ones)

These come from the original ten examples and Snake confirms they generalize:

- `scene` blocks with `var`-typed fields work as a top-level container for game state.
- `state` blocks inside `scene` (not just inside `ai`) work — states are a general control-flow mechanism, not RPG-AI-specific.
- `every <duration>:` works at the top level of a state, not just inside `ai` declarations. This was visible in Example 10 (boss attack telegraphs) but Snake confirms it.
- `on <event>:` handlers are state-scoped and deregister on transition (the safety property from Example 4 saves us from input bleeding between `playing` and `game_over`).
- `->` transitions still work the same way.
- Tuple arithmetic: `snake[0] + direction` adds two `(int, int)` tuples component-wise. This was implied by Example 1's sprite math (`hero.x += 200 * dt`) and Example 8's `(0, 4, -8)`.
- String interpolation: `"Score: {score}"` was specified in `06-design-document.md` §2.5.2 and Snake is the first example to use it.

---

## New design pressures (the actual point of this example)

Snake forces decisions the original ten did not. Each one is flagged here so the design document can absorb it.

### NP1. Press-vs-hold input distinction

The original ten examples only show `if key.right: ...` inside `on update(dt)` — that is "while held" semantics. Snake needs **press-once** semantics: pressing right once should change direction once, not flood the buffer.

**Decision:** introduce `key_press.<name>` as a distinct event source from `key.<name>`. The two coexist:

- `key.right` is a boolean property — true while held. Used in continuous-update code.
- `on key_press.right:` is an edge event — fires once on the down-stroke. Used in event-driven code.

Symmetrically: `key_release.<name>` for the up-stroke. Likely needed eventually; defer until something requires it.

**Design doc impact:** §6 stdlib needs an `input` section that documents this distinction. The `input` table now has three event sources: `key`, `key_press`, `key_release`.

### NP2. List operations

Snake needs `prepend`, `pop_back`, and `append` on a list. These are not exotic but the original ten only used lists in declarative contexts (`random.choice([...])`).

**Decision:** the `array of T` / `list of T` type ships with at minimum:

- `.append(x)` — add to end
- `.prepend(x)` — add to front
- `.pop_back() -> T` — remove and return last
- `.pop_front() -> T` — remove and return first
- `.length` — property, not method
- Indexing with `[i]` (0-based, panics on out-of-bounds)
- Slicing with `[i:j]` (copies)
- `.contains(x)` and `x in list` (these are equivalent)

**Design doc impact:** §7 stdlib `core` section needs explicit list method documentation.

### NP3. List comprehensions, or not?

The natural way to write `random_free_cell` is a list comprehension:

```twe
let free = [(x, y) for x in 0..<grid_size.x
                   for y in 0..<grid_size.y
                   if (x, y) not in snake and (x, y) != food]
```

The Snake code above uses imperative loops instead, which is uglier but doesn't require a new language feature.

**Decision:** **defer list comprehensions to v0.2.** Reasoning: they're delightful but they're a pure ergonomic win, not a capability win. The imperative version works, and v0.1 is already large. Add comprehensions only after Phase 2's vertical-slice game proves real friction. (If list comprehensions show up wanted in 5+ places during Phase 2, promote to v0.1.)

**Design doc impact:** add to §11 future work.

### NP4. Set membership and set operations

Snake's `cell in snake` is list membership (O(n)) — fine for a 60-cell snake. But the natural extension for "free cells" wants `cell in occupied_set` (O(1)).

The original ten examples use `set of T` only abstractly. Snake doesn't strictly require a `set` type, but a real implementation of larger grid games would.

**Decision:** keep `set of T` in v0.1 with only the basics: construction (`{1, 2, 3}`), `.contains`, `.add`, `.remove`, `in` operator. No set-algebra operators (`+`, `-`, `&`, `|`) in v0.1; use methods if needed (`a.union(b)`).

**Design doc impact:** clarification in §6 — set literal syntax is `{}` (with at least one element to disambiguate from empty-block), or `set()` for empty.

### NP5. Drawing primitives — `rect` and `text`

The original ten use `sprite`, `visual` (procedural), and `particles`. None of them draw a flat-colored rectangle or render UI text. Snake needs both.

**Decision:** add a `draw` stdlib module with primitive shapes:

- `rect(at: vector, size: vector, color: color, fill: bool = true)`
- `circle(at: vector, radius: length, color: color, fill: bool = true)`
- `line(from: vector, to: vector, color: color, width: length = 1px)`
- `text(content: string, at: vector, color: color = color.white, size: length = 16px, align: text_align = left)`

These are only legal inside `on render():` handlers. Calling them outside `on render()` is a runtime error (or a strict-mode compile error).

**Design doc impact:** §7 needs a `draw` module. Also adds a new event: `on render():`, distinct from `on update(dt):`. Update is for state mutation; render is for drawing. The split is important for fixed-timestep physics in v0.2+.

### NP6. The `screen` ambient resource

Snake uses `screen.center`. The original ten never queried screen properties.

**Decision:** `screen` is an ambient resource (like `time`, `key`, `scene`) providing:

- `screen.size: vector` — current viewport size in pixels
- `screen.center: vector` — `screen.size / 2`
- `screen.dpi_scale: float` — for high-DPI displays

**Design doc impact:** §7 stdlib gains a `screen` section.

### NP7. Coordinate space conversion: cells to pixels

Snake stores positions in cell coordinates (`(10, 7)`) but draws in pixels (`cell * cell_size`). The conversion is manual.

**Decision:** keep it manual in v0.1. A `grid` primitive that hides this conversion is tempting but adds another keyword and the user can do it in three characters. Revisit in v0.2 if many games need grids.

This is an explicit choice to **prefer composability over magic**. The user multiplies two values; nothing surprising happens.

### NP8. Restart / reset pattern

Snake's `reset()` method mutates scene state then `-> playing`. This is a very common pattern (every game has a "new game" button).

**Decision:** no special syntax in v0.1. The pattern works fine using a regular method. If Phase 2's vertical slice shows the pattern repeatedly, consider syntactic sugar like `restart` as a state transition that re-runs `initial`.

### NP9. State entry/exit hooks

The original `state` design (Example 4) implicitly runs the state body on entry. Snake's `game_over` state has only handlers — no entry body — and that's fine. But one design question lurks:

Should there be an explicit `on enter:` / `on exit:` hook inside states, distinct from the body?

```twe
state game_over:
    on enter:
        sound.play("game_over.wav")
        # ...
    on exit:
        # cleanup
```

Currently the body of a state runs on entry, but there's no clean way to say "this code runs on entry and is distinct from event handlers."

**Decision:** keep v0.1 simple — body code runs on entry, that's it. If `on enter:` / `on exit:` shows up wanted in 3+ examples during Phase 2, promote to v0.1.

**Design doc impact:** flag in §11 future work.

### NP10. Tuple-typed list elements need explicit annotation

The line `var snake: list of (int, int) = [(10, 7), ...]` requires Twe's type system to handle list-of-tuple. The original ten examples used homogeneous lists of simple types (`list of Item`) or unannotated.

**Decision:** the type system handles structural tuple types. `(int, int)` is a valid type expression — confirmed in §3.7 of the design doc but Snake is the first example to use it concretely.

---

## Summary: what Snake changed

After absorbing Snake, the design document has the following deltas:

| Section | Change |
|---------|--------|
| §6 built-in types | Confirm `set of T` semantics; clarify set literal syntax |
| §7 stdlib | Add `input` section with `key` / `key_press` / `key_release` distinction |
| §7 stdlib | Add `draw` section with `rect`, `circle`, `line`, `text` |
| §7 stdlib | Add `screen` ambient resource |
| §7 stdlib | Document list methods explicitly (`.prepend`, `.pop_back`, etc.) |
| §4.10 events | Add `on render():` as distinct from `on update(dt):` |
| §3.3 grammar | `state` blocks are valid inside `scene`, not just `ai` (already true; emphasize) |
| §11 future work | List comprehensions; `on enter:` / `on exit:` state hooks; grid coordinate primitive |

None of these are alarming. They're the kind of additions that should fall out of every concrete example, and the fact that 10 examples missed them is precisely why Snake was a useful pressure test.

---

## What Snake did *not* break

It's worth listing what survived contact with Snake unchanged:

- The principle of "game concepts as first-class blocks" held. `scene` and `state` cleanly host the game.
- Single inheritance / declarative blocks were not stressed (Snake doesn't need either).
- Coroutines / fibers were not stressed. Snake is event-driven; it doesn't `wait`.
- The type system held. Inferred types worked for everything except the heterogeneous-tuple list, which had a clean annotation.
- The 6 core declarative blocks ship list (`entity`, `state`, `visual`, `particles`, `scene`, `dialogue`) was not expanded by Snake. **Good** — it means the core is sized correctly.

---

## When this example becomes runnable

Per `05-roadmap.md`:

- **End of Phase 1** (~week 8): an ASCII / terminal version runs (no `rect`, `text`, `screen`, but the logic and state machine work).
- **End of Phase 2** (~week 14): the full graphical version above runs at 60fps. **This is the realistic answer to "when can I make Snake in Twe."**
- **Phase 3+**: the same code runs on the bytecode VM, considerably faster (irrelevant for Snake but free).

Snake is also the recommended **warm-up game** before the Phase 2 Vampire Survivors clone. Two days of building Snake will reveal interpreter bugs and ergonomics issues that would otherwise hide for weeks.

---

## A note on this example's role

This file is a model for what should happen with *every* future example added to Twe before v1.0:

1. Write the program as the user would want to write it.
2. Identify what the existing language and stdlib already support.
3. Identify the gaps — explicitly, in a numbered list.
4. For each gap, decide: ship it in v0.1, defer to a later version, or reject.
5. Update the design document.

This is the same process that produced the original ten. Snake is the eleventh, and it earned its place by surfacing ten distinct design pressures that the original ten missed. Future examples should clear a similar bar before being added.
