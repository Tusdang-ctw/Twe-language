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

# Part II — Three games

The first half of this tutorial covered the pieces. The rest of it builds three games end-to-end. Each chapter ends with a runnable file in `examples/`; if you get stuck, diff your file against the reference.

The chapters get progressively bigger:

- **Pong** — paddles, a ball, a score. About 200 lines. Touches everything in Part I.
- **Survivors** — the Vampire-Survivors clone we ship in `examples/survive_beta/`. Entities, waves, weapons, level-up, save-load. Around 1300 lines; we'll read it, not type it.
- **Mini-RPG** — dialogue trees, scenes, gradual typing in anger. Smaller than Survivors but uses the parts neither of the other two reach.

If you only have an hour, do Pong. If you have an afternoon, do Pong and Survivors.

## Building Pong

Pong is a deceptively rich first game: it has input, real-time physics, AI, scoring, and a state machine that loops. Every piece you'll need for a bigger game is here in miniature. We'll write `examples/pong.twe` from scratch.

### The scaffold

Open a new file `pong.twe` and start with the dimensions and constants:

```twe
let view_w = 640.0
let view_h = 480.0

let paddle_w = 12.0
let paddle_h = 80.0
let paddle_speed = 360.0

let ball_size = 10.0
let ball_speed = 320.0

let win_score = 5
```

Top-level `let` bindings are immutable — declare-once constants. Pong has a small handful, so they live at the top of the file rather than inside the scene. The 640×480 viewport matches the default `twec play` window.

Next, the scene shell:

```twe
scene Pong:
    var left_y = 200.0
    var right_y = 200.0
    var ball_x = 320.0
    var ball_y = 240.0
    var ball_vx = 320.0
    var ball_vy = 192.0
    var left_score = 0
    var right_score = 0
    var serve_timer = 0.0

    initial: playing

    state playing:
        on render():
            text("pong", at: (300, 220), size: 24, color: color.white)
```

`scene Pong:` opens the scene declaration. Each `var` line declares a mutable field — these are the only places we can store changing state in Pong. (`let` would refuse mutation; we'd never be able to move the paddle.)

A subtlety v0.1 enforces: **scene field defaults must be literal constants**. We can't write `var left_y = view_h * 0.5 - paddle_h * 0.5` here even though the math is constant — the bytecode VM rejects non-literal initializers. So we hard-code the centered values: `200.0` is `(480 - 80) / 2`, `320.0` is `640 / 2`, etc. The `let` constants up top still drive the rest of the file.

`initial: playing` says the scene boots in the `playing` state. `state playing:` declares it. The `on render():` block is what fires every frame in the play window. Run it now:

```
twec play pong.twe
```

You should see "pong" centered on a black 640×480 window. We have a working scene; the rest is filling in behavior.

### Paddle input

Pong's left paddle responds to W/S. Add an `on update(dt):` block to the `playing` state, above `on render`:

```twe
    state playing:
        on update(dt):
            if key.w:
                left_y -= paddle_speed * dt
            if key.s:
                left_y += paddle_speed * dt
            if left_y < 0.0:
                left_y = 0.0
            if left_y > view_h - paddle_h:
                left_y = view_h - paddle_h
```

`on update(dt):` runs every simulation tick with the real-time delta. `key.w` reads the W key as a held bool — `true` while the player is holding it down, `false` when released. Multiplying speed by `dt` makes the motion frame-rate-independent: 360 units per second whether your machine renders at 60 fps or 144.

The two clamps after pin the paddle to the playfield: `left_y = 0.0` if it tries to leave the top, `view_h - paddle_h` if it tries to leave the bottom. Without them the paddle drifts off-screen.

Now draw the paddle. Replace the placeholder `text(...)` in `on render()`:

```twe
        on render():
            rect(at: (0.0, left_y), size: (paddle_w, paddle_h), color: color.white)
```

`rect(at:, size:, color:)` is one of the core stdlib drawing primitives. Coordinates are top-left-origin, +y down. Re-run `twec play pong.twe` — you should see a white paddle on the left edge that responds to W/S.

### The AI paddle

The right paddle should track the ball. Add to `on update(dt):` after the player block:

```twe
            let target = ball_y - paddle_h * 0.5
            let ai_speed = paddle_speed * 0.8
            if right_y < target:
                right_y += ai_speed * dt
                if right_y > target:
                    right_y = target
            elif right_y > target:
                right_y -= ai_speed * dt
                if right_y < target:
                    right_y = target
            if right_y < 0.0:
                right_y = 0.0
            if right_y > view_h - paddle_h:
                right_y = view_h - paddle_h
```

`target` is the y the paddle would need to be at to put the ball at its center. The AI moves toward `target` at 80% of the player's speed — a perfect tracker is unbeatable, and 80% is the genre-canonical "fair but firm" feel. The inner `if` guards prevent overshoot when the AI is close enough to snap to target this frame.

And draw it. Append to `on render()`:

```twe
            rect(
                at: (view_w - paddle_w, right_y),
                size: (paddle_w, paddle_h),
                color: color.white,
            )
```

Note the trailing comma after `color.white` — Twe accepts (and `twec fmt` keeps) trailing commas in multi-line lists. They make the next addition a one-line diff.

### The ball

Ball physics are velocity-integration plus four collision tests. Append to `on update(dt):`:

```twe
            ball_x += ball_vx * dt
            ball_y += ball_vy * dt

            if ball_y < 0.0:
                ball_y = 0.0
                ball_vy = -ball_vy
            if ball_y > view_h - ball_size:
                ball_y = view_h - ball_size
                ball_vy = -ball_vy
```

The first two lines are Euler integration: position += velocity × time. The next four lines are wall bounces — when the ball passes the top or bottom edge, snap it back inside the wall and flip the y-velocity sign. Snapping is important: without it the ball can stay overlapping the wall on the next frame, flip its velocity again, and get stuck oscillating in place.

For paddle collisions, we'll do AABB overlap (Axis-Aligned Bounding Box):

```twe
            if ball_x < paddle_w and ball_vx < 0.0:
                if ball_y + ball_size > left_y and ball_y < left_y + paddle_h:
                    ball_x = paddle_w
                    ball_vx = -ball_vx
                    let offset = (ball_y + ball_size * 0.5) - (left_y + paddle_h * 0.5)
                    ball_vy = offset * 6.0

            if ball_x > view_w - paddle_w - ball_size and ball_vx > 0.0:
                if ball_y + ball_size > right_y and ball_y < right_y + paddle_h:
                    ball_x = view_w - paddle_w - ball_size
                    ball_vx = -ball_vx
                    let offset = (ball_y + ball_size * 0.5) - (right_y + paddle_h * 0.5)
                    ball_vy = offset * 6.0
```

Two things are happening per paddle. The outer `if` checks "is the ball inside the paddle's x-column AND moving toward it?" — the velocity check stops the ball from re-colliding on the frame *after* a successful bounce. The inner `if` checks the y-overlap. If both pass: snap the ball out, flip x-velocity, and **adjust y-velocity by the offset from paddle center**. That last line is what makes Pong skill-based: you aim by hitting the ball with the edge of your paddle.

Draw the ball:

```twe
            rect(
                at: (ball_x, ball_y),
                size: (ball_size, ball_size),
                color: color.white,
            )
```

Run it. The ball should bounce off paddles and walls and (eventually) escape past one of the paddles into the void. We're missing the score reset.

### Scoring and the `scored` state

When the ball leaves the playfield, we want a brief pause before the next serve so the player registers the point. That's a second state. Append to the bottom of the scene:

```twe
    state scored:
        on update(dt):
            serve_timer -= dt
            if serve_timer <= 0.0:
                serve_timer = 0.8
                ball_x = 315.0
                ball_y = 235.0
                if right_score > left_score:
                    ball_vx = -ball_speed
                else:
                    ball_vx = ball_speed
                ball_vy = ball_speed * 0.6
                if left_score >= win_score or right_score >= win_score:
                    -> game_over
                else:
                    -> playing

        on render():
            rect(at: (0.0, left_y), size: (paddle_w, paddle_h), color: color.white)
            rect(
                at: (view_w - paddle_w, right_y),
                size: (paddle_w, paddle_h),
                color: color.white,
            )
            text("POINT", at: (270, 220), size: 36, color: color.yellow)
```

`scored` ticks down `serve_timer` until it hits zero, then resets the ball, picks a serve direction (toward whoever just lost), and either ends the game (if someone hit the win threshold) or hops back to `playing`. The `->` token is a state transition: `-> game_over` schedules a transition that fires after the current handler returns.

Now wire the scoring transitions in `playing.on_update`. Append at the end:

```twe
            if ball_x < -ball_size:
                right_score += 1
                -> scored
            if ball_x > view_w:
                left_score += 1
                -> scored
```

And display the scores in `playing.on_render`:

```twe
            text("{left_score}", at: (160, 60), size: 48, color: color.white)
            text("{right_score}", at: (456, 60), size: 48, color: color.white)
```

`"{left_score}"` is a string interpolation — Twe builds the formatted string at runtime. Not `format!()`, not `+`-concatenation, not `printf` — interpolation is part of literal syntax.

### `game_over` and restart

The last state. Append:

```twe
    state game_over:
        on update(dt):
            if key_press.r:
                left_score = 0
                right_score = 0
                ball_x = 320.0
                ball_y = 240.0
                ball_vx = 320.0
                ball_vy = 192.0
                -> playing

        on render():
            rect(at: (0, 0), size: (view_w, view_h), color: color.black)
            if left_score >= win_score:
                text("You win!", at: (230, 180), size: 48, color: color.green)
            else:
                text("AI wins", at: (240, 180), size: 48, color: color.red)
            text(
                "Final  {left_score} - {right_score}",
                at: (230, 250),
                size: 24,
                color: color.white,
            )
            text(
                "Press R to play again",
                at: (210, 320),
                size: 18,
                color: color.gray,
            )
```

Two things to call out. `key_press.r` (not `key.r`) is edge-triggered — `true` only on the single frame the key was first pressed. We use it for one-shot actions like "restart"; `key.*` is for held input like paddle movement. Mixing them up is a common Twe footgun: held-`r` would restart-loop forever as long as the player held R.

The opaque black rect draws first to dim the playfield behind the end-screen text.

### What you've shipped

You've built a complete Pong in a few hundred lines: input, real-time physics, AI, scoring, win condition, restart. Some things to notice:

- **No setup, no event loop, no allocator.** The scene is the unit. State machines are the control flow.
- **No nullable types, no `Option`.** Every `var` always has a value because the field default is required.
- **No type annotations needed.** The inferer figured out everything from your initial values. You can add `: float` etc. and they'll be checked, but the bare program runs.
- **Hot reload works.** Edit `pong.twe` while the window is open and the change appears next frame. Your scores reset because the scene is rebooted, but the game state is captured in scene fields — a future session could persist them across reloads.

The reference file is in [`examples/pong.twe`](../examples/pong.twe). If yours differs, diff and figure out which version you prefer.

## Reading Survivors

Now we step up. [`examples/survive_beta/main.twe`](../examples/survive_beta/main.twe) is a 1264-line Vampire-Survivors clone — the production beta we ship with v0.8 and the first-party game targeting itch.io. Open the file in your editor and follow along; we won't retype it, we'll *read* it.

If Pong taught you the shape, Survivors teaches you the surface area: entities, waves, weapons that compose, level-up trees, save/load, gamepad, particles, pause stack. Every Phase 8 → Phase 13 feature is in here.

### The shape, from 30,000 feet

```
settings + persistent globals    (lines  1– 98)
upgrade pool (top-level fns)     (      99–172)
entity Slime                     (     173–226)
entity Bat                       (     227–276)
entity Skeleton + SkeletonBolt   (     277–365)
entity Boss                      (     366–424)
entity Projectile                (     425–523)
entity Blade                     (     524–590)
entity Aura                      (     591–676)
entity Spark                     (     677–712)
entity XPGem                     (     713–787)
scene SurviveBeta                (     788–end)
  state playing
  state paused
  state level_up
  state game_over
```

That's it. **One scene, ten entities, four states.** The complexity is in how they compose, not in any individual piece. Pong's whole architecture survives here — it's just been split across more files of behavior.

### Globals and why they exist

Lines 33–87 declare the world state: `arena_w`, `player_x`, `player_hp`, `xp`, `level`, `attack_interval`, `magnetic_radius`, etc. These are **top-level `var`s** — file-scope mutable bindings.

Why aren't these scene fields like Pong's `left_y`? Two reasons:

1. **Top-level functions can't reach scene fields.** `apply_upgrade(id)` (line 125) is a free function called from the level-up modal. It needs to bump `attack_interval`. Free functions can't see `scene.field` — they can see top-level bindings. So `attack_interval` lives at the top.
2. **Entities can't reach scene fields either.** A `Slime`'s `update(dt)` reads `player_x` to chase. If `player_x` were a scene field, the slime would need a back-reference to the scene. Top-level `var` is the simpler path.

This is the v0.1 trade-off: scenes have their own data, but cross-cutting state lives at file scope. For a game this size it's manageable; if it grew to ten files we'd want module-scoped state, which is what Phase 13 lays groundwork for.

### Entities are just classes with `update` and `render`

Look at `entity Slime:` at line 173. It's three things:

```twe
entity Slime:
    var pos = (0.0, 0.0)
    var hp = 1
    # ...

    function update(dt):
        if in_levelup:
            return
        # chase player; check contact damage
        # ...

    function render():
        if in_levelup:
            return
        rect(at: ..., size: (20, 20), color: color.cyan)
```

That's the whole entity contract: **fields + update(dt) + render()**. The runtime spawns instances via `spawn Slime at (x, y)`, ticks each one's `update` per frame, and draws each one's `render`. There's no virtual dispatch table to register, no `extends GameObject`. The compiler sees `entity Slime:` and that's the entire ceremony.

`entities.of(Slime)` is how you iterate them — `for slime in entities.of(Slime):` works in any context. It's not a list; it's a query. The runtime maintains the index for free.

### Spawning, despawning, and death hooks

Look at the wave spawner inside `scene SurviveBeta` (around lines 830–880). It picks an enemy class and:

```twe
spawn Slime at (sx, sy)
```

`spawn` returns the new instance; `at` is the only required keyword for entity spawn. Fields default to whatever `var` initializers the entity declared.

Despawning happens with `despawn self` (inside the entity's own method) or `despawn slime` (from outside). Once despawned, the runtime stops ticking the entity and skips it in `entities.of(...)` queries.

What about effects when something dies? Survivors uses a **death hook**:

```twe
on Slime.death(s):
    spawn Spark at (s.pos.x, s.pos.y)
```

The hook fires on every despawn. The argument `s` is the slime instance just before it goes away — its fields are still valid. This is how a Spark particle gets spawned without coupling the spark logic to the kill site (the projectile, the blade, the aura, the boss bullet — every kill path automatically gets the effect).

### Composing weapons

Pong had one collision check. Survivors has three weapons and four enemy classes — twelve interaction pairs. The pattern that keeps it manageable:

> **Each weapon owns its own collision pass against every enemy class.**

Look at `Projectile.update` (around line 432). It iterates `entities.of(Slime)`, then `entities.of(Bat)`, then `entities.of(Skeleton)`, then `entities.of(Boss)` — four overlap checks per projectile, despawning on the first hit. The `Blade` does the same; the `Aura` does the same.

This duplicates loops, but the duplication is honest: when you add a new enemy, you grep for `entities.of(` and add it to every weapon. That's a deliberate friction — it forces you to think about whether the new enemy should actually take damage from each weapon (and a boss often shouldn't from a one-shot projectile, but should from an aura tick).

Cleverer dispatch — a `damageable` interface, a base `Enemy` class, a global event bus — is a footgun at this scale. Per Principle 2 (one obvious way), Twe doesn't ship the abstractions to express it.

### The level-up modal pattern

This is the most interesting state in the file. `state level_up:` has only an `on render():` block. No update, no every-clock. Yet it's the screen the player interacts with most.

The pattern:

```twe
state level_up:
    on render():
        rect(at: (0, 0), size: (view_w, view_h), color: color.black)
        text("LEVEL UP!  (Lv {level})", at: ..., size: 28, color: color.yellow)
        if button(at: (160, 150), size: (320, 50), label: upgrade_name(pick_a)):
            apply_upgrade(pick_a)
            in_levelup = false
            -> playing
```

Two things to notice:

1. **`button(...)` is an immediate-mode widget** — it draws AND hit-tests AND returns truthy on click, all in one call from inside `on render`. There's no `on_click` callback to register. The whole pause menu, the whole upgrade picker, the keybind UI in `examples/keybind_demo.twe` — all of them are inline in render.
2. **State transitions inside `on render` are honored** — the engine applies `-> playing` after the render block returns. (This was historically broken for modal-state buttons; v0.8 session 13 fixed it.) The pause menu uses the same pattern.

`in_levelup = false` is a top-level flag every entity reads. It's redundant with the state transition — in_levelup is true while the modal is open, false otherwise — but it lets entities gate their `update` without checking which state the scene is in. It's the cheap way to say "freeze the world while the picker is open."

### Save and load

Lines 18–26 set up keybind defaults; line 26 calls `settings.try_load(...)`:

```twe
settings.set_default("keys.right", "right")
# ...
settings.try_load("examples/survive_beta/survive_beta.save")
```

`settings` is the Phase 10 stdlib facility for persistent named values. `set_default(key, value)` registers a fallback; `try_load(path)` reads a save file that may or may not exist (silently ignores missing). Inside the pause menu, the "Save Bindings" button calls `settings.save(path)` to write the current values.

This is the bottom layer of the save system — key/value persistence. Full structured save (snapshot the entire game state) is the `save SaveSlot:` block syntax that lands in v0.3+; v0.8 ships only the layer Survivors actually needs.

### Pausing

`pause(true)` is an engine primitive that halts every-clock and entity ticks. The `paused` state's `on render` fires (so the menu can draw and accept input), but no other behavior runs — slimes freeze, bolts hang in mid-air, the wave spawner stops counting.

```twe
if key_pressed("escape") or gamepad_press.b:
    pause(true)
    -> paused
```

Note `key_pressed(name)` — a runtime dispatch by name string — versus Pong's `key_press.r`, an edge-triggered field on the `key_press` ambient. Both work. `key_pressed(name)` is the path you use when the binding comes from a settings file (`settings.get("keys.up")` is a string at runtime).

### The bigger lesson

Survivors looks intimidating because it's long, but every section is one of a handful of patterns:

- **Top-level `var` for cross-cutting world state.** Not best practice in a 50-class engine; the right call for a one-file game.
- **`entity` for anything with its own update/render lifecycle.** Even purely visual things (Spark) earn an entity if they have their own lifetime.
- **`entities.of(Class)` for queries** — read-only, multi-class loops are how weapons interact with enemies.
- **One scene, many states.** Each state is a screen: playing, paused, level-up, game-over. Buttons and key handlers are inline in `on render` / `on update`.
- **Top-level functions for things that mutate top-level state.** `apply_upgrade(id)` works because `attack_interval` is a top-level `var`.

If you can read this file — really read it, line by line — you can write it. We'll do a smaller third game next.

## Building a mini-RPG

The third chapter is the smallest. We'll write a tiny dialogue script using Twe's `dialogue` block — a feature neither Pong nor Survivors touched. The reference file is [`examples/dialogue_demo.twe`](../examples/dialogue_demo.twe).

A `dialogue` block is a top-level callable that runs a sequenced script: `say` lines and `choice:` branches. It compiles like a function and you invoke it by name.

```twe
dialogue Greet:
    say "You enter the Stag's Head. The bartender looks up."
    say "Bartender": "Bit late for travelers. What'll it be?"
    choice:
        "Ale":
            say "Bartender": "On the house. Stay safe out there."
            say "The ale is warm. The fire is warmer."
        "Information about the cave to the north":
            say "Bartender": "Don't go. Last party didn't come back."
            say "He won't say more."
        "Just looking around":
            say "Bartender": "Suit yourself."

Greet()
```

Save and run with `twec run examples/dialogue_demo.twe` (headless — the dialogue runtime prints to stdout in v0.8). You'll see the bartender greet you, the choice list, and the first branch's lines. v0.1 picks the first branch deterministically — interactive selection rides the UI surface, but the dialogue runtime doesn't wire to it yet. To follow a different branch, comment the choices above the one you want.

What's happening line-by-line:

- `dialogue Greet:` declares a top-level callable named `Greet`. The body runs when you call `Greet()`.
- `say <text>` prints a line.
- `say <Actor>: <text>` prefixes with the actor's name. The actor can be a string literal (`"Bartender"`) or any expression that evaluates to a string or instance — for an instance, the class name is used.
- `choice:` lists branches; each branch's body is its indented block.

That's the whole dialogue surface for v0.8: `say`, `say <actor>:`, `choice:`. It's intentionally austere. Two limits to know about now:

- **`wait` inside a dialogue body is a runtime error in v0.8.** Dialogue blocks don't yet integrate with the fiber scheduler. (They will — once the UI surface for interactive choices lands, the engine will need fiber suspension anyway.)
- **`dialogue:` is a top-level declaration only.** You can't nest one inside a `state` block. To run a dialogue from a state, declare the dialogue at file scope and call it: `Greet()` from inside an `on update(dt):` body. The first call advances the script.

For a real RPG you'd compose the dialogue with the rest of the surface: an inventory `var`, a `quests` list of strings, persistent flags via `settings.set("flags.met_bartender", true)`, a tilemap world to walk between scenes. The dialogue block carries the actor lines and the branching; everything else is the surface we already covered.

## What you've shipped

Three games — Pong from scratch, Survivors as a reading exercise, a mini-RPG to use the dialogue block. Combined, they exercise: scenes, states, transitions, `update` / `render`, `key.*` and `key_press.*` and `key_pressed(name)`, gamepad, `every-clock`s, `wait`, entities + `update` + `render` + `entities.of(...)`, `spawn` / `despawn`, death hooks, top-level functions, top-level `var`s, the immediate-mode widget set, `settings`, `pause(...)`, `dialogue:` + `say` + `choice:`, and string interpolation.

What's left? Modules (covered in `examples/modular_*_demo/`), strict + verified type modes (`docs/02-type-system.md`), procedural visuals (`examples/visual_fire.twe`), tilemaps and physics for serious games. Each is a self-contained surface; pick the one your next project needs.

## Where to go next

- **`docs/01-examples.md`** — the eleven example programs the language design is held against.
- **`docs/06-design-document.md`** — the formal grammar + semantics. Read this when you want to know exactly what Twe does in a corner case.
- **`docs/02-type-system.md`** — the gradual / strict / verified type-system design.
- **`docs/03-runtime.md`** — the runtime architecture and the explicit list of footguns Twe doesn't replicate (per-language pitfalls from Lua, Wren, GDScript).
- **`docs/05-roadmap.md`** — the phase-by-phase plan from "design document" through v1.0.

Have fun.

