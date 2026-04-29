# The Twe Tutorial

> A first-hour walkthrough that ends with a small playable game. Read it top to bottom; copy each snippet and run it before continuing. Twe v0.1.

## What Twe is

Twe is a language for writing small games. Its design audience is two sets of authors at once — humans who want a game running quickly, and language models that need a grammar they can generate without ambiguity. The constructs that make up a game (`scene`, `state`, `entity`, `dialogue`, `particles`) are first-class language features, not library APIs you have to discover.

This tutorial assumes you've installed Twe and can run `twec` on your terminal. If `twec version` prints something, you're set.

## Hello, scene

Save this to `hello.twe`:

```twe
scene Greeter:
    var ticks: int = 0

    initial: counting

    state counting:
        every 100ms:
            ticks += 1
            print("tick {ticks}")
            if ticks >= 3:
                -> done

    state done:
        print("done")
```

Run it headless: `twec run --frames 30 hello.twe`. You should see `tick 1` through `tick 3` and then `done`.

Three concepts just landed without ceremony.

A `scene` is the unit Twe ticks each frame. `Greeter` declares one with a single mutable field, `ticks`. The `initial:` line names the state the scene boots in. Every frame, the runtime advances the scene's clocks.

A `state` is a labeled chunk of behavior the scene can be in. `every 100ms:` is a *recurring action* attached to the state — it fires whenever 100ms has accumulated. The `-> done` is a *transition*: the scene leaves `counting` and enters `done`. From that point on, only `done`'s blocks fire.

`print(...)` and string interpolation `"tick {ticks}"` are both regular stdlib features. Note no `format!` or `print()` boilerplate — interpolation is part of string literal syntax.

## Mutability and the two binding forms

Twe has two binding keywords: `let` (immutable) and `var` (mutable). A scene field declared `var ticks: int = 0` is mutable; subsequent `ticks += 1` work. A `let` binding is final; reassigning fails to parse.

Type annotations on bindings (`: int`) are optional. Without them, the inferer figures out the type from the initial value. They become load-bearing only in *strict mode* — see below.

```twe
let answer = 42        # inferred int, immutable
var counter = 0        # inferred int, mutable
let pi: float = 3.14   # annotated float, immutable
```

## Frame events: `on update(dt):` and `on render():`

Twe has two top-level event blocks for per-frame work:

```twe
var t = 0.0

on update(dt):
    t = t + dt

on render():
    print("t = {t}")
```

`on update(dt):` fires every simulation tick with the real frame delta. This is where state changes go.

`on render():` fires once per *rendered* frame. In `twec run` (headless) it doesn't fire; in `twec play3d` (the wgpu loop), it's where you call `cube(...)` and other drawing primitives.

Don't read input or mutate the world from `on render():` — keep those in `on update`. The inverse holds too: don't draw from `on update`. The split keeps reasoning local.

## Input: `key.*`

The `key` ambient object exposes named keys as bools. Hold the key, the field is `true`; release, it's `false`. There's a sibling `key_press.*` that's edge-triggered — only `true` for the single frame the key was first pressed.

```twe
on update(dt):
    if key.right:
        print("→")
    if key_press.space:
        print("space pressed")
```

Available names in v0.1: `right`, `left`, `up`, `down`, `space`, `escape`, `enter`, `r`, `w`, `a`, `s`, `d`. The same surface works under `twec play` (macroquad, 2D) and `twec play3d` (wgpu, 3D).

## Entities and ECS-shaped queries

Behavior that's about a *thing* — a player, a bullet, a monster — goes in an `entity`:

```twe
entity Bullet:
    var x: float = 0.0
    var y: float = 0.0
    var dx: float = 0.0
    var dy: float = 0.0

    initial: flying

    state flying:
        on update(dt):
            x += dx * dt
            y += dy * dt
            if y < 0:
                despawn self

let bullet = Bullet()
bullet.x = 100.0
bullet.dy = -200.0
spawn bullet
```

`spawn <expr>` registers an instance with the runtime; it ticks every frame until `despawn self` removes it. `entities.of(Bullet)` returns a list of all live `Bullet` instances. `entities.count(Bullet)` returns the count.

The `state` machinery you saw on `scene` works the same on entities. An entity's state has its own `every`, `on update`, `on render`, `on key_press`, plus the predicate handlers below.

## Predicate hooks

Inside a state, `on <expr>:` is an edge-triggered handler. The runtime evaluates the expression every frame; when it transitions from false to true, the body fires. It does *not* re-fire while the predicate stays true — that's the point.

```twe
entity Goblin:
    var hp: int = 100

    initial: chase

    state chase:
        every 100ms:
            hp -= 25
        on hp <= 30:
            -> flee

    state flee:
        every 100ms:
            hp -= 50
        on hp <= 0:
            -> dead

    state dead:
```

The predicate fires once per false→true transition. State-machine AI written this way avoids the classic "did I check this last frame?" boilerplate — the runtime tracks it.

## Cooperative `wait`

Inside a state's on-entry sequence (the bare statements at the top of the state body), `wait <duration>` suspends. While suspended, the state's clocks and on-update pause. After the duration elapses, the body resumes from the next statement.

```twe
state alert:
    play_animation("alert")
    wait 0.5s
    -> chase
```

`wait` only works as a *direct* statement of a state body in v0.1. Inside `if`, `while`, function calls, or dialogue blocks, you'll get a clear runtime error pointing at the limitation. The bytecode VM and the tree-walker both support state-body `wait`; pick whichever interpreter you prefer (`--vm tree` is the default; `--vm bytecode` is faster on hot paths).

## Dialogue

A `dialogue` block is a parameterless callable that runs a sequenced script:

```twe
dialogue Trade:
    say "A merchant approaches."
    say "Merchant": "Looking to buy?"
    choice:
        "Yes":
            say "Merchant": "Excellent."
        "No":
            say "Merchant": "Suit yourself."

Trade()
```

`say <text>` prints the text. `say <actor>: <text>` prefixes the line with the actor's name (for an `Instance` value, the class name; for a string, the string itself).

`choice:` lists branches. v0.1 picks the first branch deterministically — interactive selection lands when the UI surface is designed.

## A tiny game

Putting it all together — a state-machine AI that orbits the camera in a 3D scene. Save as `tutorial_game.twe`:

```twe
let ring_radius = 2.0
let cam_distance = 4.0
var t = 0.0

on update(dt):
    t = t + dt
    let cx = cam_distance * math.sin(t * 0.6)
    let cz = cam_distance * math.cos(t * 0.6)
    var cy = 1.5
    if key.w:
        cy = 2.5
    if key.s:
        cy = 0.5
    camera.eye = vec3(cx, cy, cz)
    camera.target = vec3(0, 0, 0)

on render():
    cube(at: vec3(0, 0, 0), color: (0.95, 0.95, 0.95, 1.0), size: 0.8)
    cube(at: vec3( ring_radius, 0, 0), color: color.red,    size: 0.5)
    cube(at: vec3(-ring_radius, 0, 0), color: color.green,  size: 0.5)
    cube(at: vec3(0, 0,  ring_radius), color: color.blue,   size: 0.5)
    cube(at: vec3(0, 0, -ring_radius), color: color.yellow, size: 0.5)
```

Run it with `twec play3d tutorial_game.twe`. A central white cube ringed by four colored cubes appears, the camera orbits, and W/S raise/lower the view height. Edit the file while running — Twe hot-reloads on save.

## Strict mode

By default, Twe's type system follows Luau's "no false positives" rule: when a constraint can't be solved, the involved type silently becomes `Unknown` and no error is reported. This keeps every program runnable, including programs that haven't been fully annotated yet.

For shipping code, you can opt a file into *strict mode*:

```twe
# strict

function add(a: int, b: int) -> int:
    return a + b

let r = add(1, 2)        # ok
let bad = add("hi", 2)   # ERROR: call argument: type mismatch — string vs int
```

The directive is a magic comment on one of the first ten lines: `# strict` (or `#! strict`, `#strict`, `#!strict`). Without it, the same program type-checks silently. With it, every unification failure becomes a diagnostic — surfaced to stderr by `twec types <file>` and inline in your editor by the LSP.

What strict mode flags today: comparison operators between mismatched types, arithmetic between non-numeric types, function-call args that disagree with parameter annotations, return values that disagree with the annotated return type, `let` annotations that disagree with the inferred value type. Annotations on classes and methods are session-3+ work; using them in v0.1 is silent (the inferer parses but doesn't yet enforce them).

## Where to go next

- **`docs/01-examples.md`** — the eleven example programs the language design is held against. Ten are working; the eleventh (Snake) is the original integration test.
- **`docs/06-design-document.md`** — the formal grammar + semantics. Read this when you want to know exactly what Twe does in a corner case.
- **`docs/02-type-system.md`** — the gradual / strict / verified type-system design. Explains the Luau influence and the three-tier model.
- **`docs/03-runtime.md`** — the runtime architecture and the explicit list of footguns Twe doesn't replicate (per-language pitfalls from Lua, Wren, GDScript).
- **`docs/05-roadmap.md`** — the phase-by-phase plan from "design document" to v0.1.

A handful of what v0.1 doesn't yet ship: `.glb` mesh import, tilemap rendering, save/load schemas, function-body `wait`, mouse input. These land in v0.2 alongside the bytecode-VM 3D path. See `notes/future-phases.md` "Carried into v0.2" for the running list.

Have fun.
