# Doc 01 — Ten Example Programs

> The most important document in this repository. Everything else flows from these ten programs (plus Snake — see `example-11-snake.md`).
>
> The grammar must support every example here. Every feature not required by these examples is suspect. When the language design is in doubt, return to these examples and check whether the answer is implied.

---

## Reading guide

Each example has three parts:

1. **The Twe code** as it should look in its target release.
2. **What it demonstrates** — the gameplay system or design intent.
3. **Implied decisions** — the syntactic and semantic commitments this example forces.

The implied decisions are the actual point. The code is the vehicle.

---

## Runtime delivery status

Not every example ships in v0.1. Some pressure-test features that require runtime work scheduled in later phases. This table is the canonical "which release runs which example."

| Example | Runtime ships in | Notes |
|---|---|---|
| 1 — Hello, sprite | **v0.1** | macroquad-backed; runs today on `twec run`. |
| 2 — Inventory & modifiers | **v0.1** (logic) | The data model runs; example doesn't drive a window. |
| 3 — Branching dialogue | **v0.1** (tree-walker only) | Tree-walker has dialogue; bytecode VM lands it in v0.3. Interactive choice (player picks) is v0.3. |
| 4 — NPC state-machine AI | **v0.1** | States, transitions, `every`, predicate hooks all ship. |
| 5 — Procedural fire | **v0.3** | `visual` block → WGSL fragment-shader compilation is Phase 9. v0.1 docs claimed this; that was wrong — the Phase 7 honesty pass demoted it. |
| 6 — Particle burst | **v0.3** (tree-walker complete; VM mirror pending) | `particles` block parses + the runtime ships on **both backends**: per-particle `on_spawn(p)` / `on_update(p, dt)` fire each frame, `p.age_ratio` auto-updates, dead particles prune, the emitter despawns when empty. `spawn ExplosionBurst at e.pos` ships too. The global event hook `on Enemy.death(e):` shipped on the **tree-walker** in Phase 9 session 7b — handlers register at top-level, fire when an instance of the named class transitions despawned → pruned, and bind the dying entity to the param. The bytecode VM mirror is a follow-on session (the compiler currently errors clearly on the construct rather than silently dropping it). |
| 7 — Save and load | **v0.2** (bottom layer; block syntax v0.3) | `save_to(path, value)` / `load_from(path)` stdlib builtins shipped in v0.2 session 4 against the `docs/07-save-system.md` design. The `save SaveSlot:` block syntax + version migration ride a follow-on session. |
| 8 — 3D camera follow | **v0.1** (cubes/spheres/`.glb`) | `play3d` runs the surface today. Polish (mouse, mat4/quat, animation) is 3D-maintenance, off the v1.0 critical path. |
| 9 — Tilemap with collision | **v0.2** (stdlib form; block syntax follow-on) | `tilemap(layout, tile_size, tiles)` + `tilemap_render` + `tilemap_at` + `tilemap_solid_at` stdlib builtins shipped in v0.2 session 6. The `tilemap Dungeon:` block syntax is a follow-on parser session. |
| 10 — Boss fight | **v0.3** | Integration test — depends on Examples 4 (states ✅) + 7 (save, v0.2 ✅) + 6 (particles, v0.3). Runs end-to-end at v0.3. |
| 11 — Snake | **v0.1** | Drove the Phase 6 NPx design pressures; runs today on `twec play`. |

If you're reading this looking for "what can I write today?", as of 2026-05-02 the answer is: Examples 1, 2, 3 (no interactive choices yet), 4, **6 (particles minus the global death-event hook)**, **7 (data round-trip via stdlib)**, 8 (cubes), **9 (tilemap via stdlib)**, and 11. Example 5 waits for v0.3's `visual` block compiler; Example 10 waits for the death-event hook + visual block. The roadmap to bring the rest online is `docs/05-roadmap.md` Phases 9 onward.

---

## Example 1 — Hello, sprite (the "first five seconds" test)

```twe
let hero = load("hero.png")
hero.pos = (200, 150)

on update(dt):
    if key.right: hero.x += 200 * dt
    if key.left:  hero.x -= 200 * dt
    if key.up:    hero.y -= 200 * dt
    if key.down:  hero.y += 200 * dt
```

**Demonstrates:** the absolute minimum program. The user types this, hits run, and sees something move. This is the PICO-8 lesson — eliminate boilerplate.

**Implied decisions:**

- No `main` function, no scene, no init. Top-level code runs at startup.
- `let` introduces an immutable binding; `hero`'s type is inferred from `load`'s return value (gradual typing per `02-type-system.md`). `sprite` is a built-in type name, not a reserved keyword (see `changes/2026-04-27-sprite-is-a-type-not-a-keyword.md`). `key` is ambient stdlib context, not a library import.
- Tuples are first-class (`(200, 150)`); they auto-coerce to `Vector2`. Member access via `.x` and `.y` works on tuples directly.
- `on update(dt):` is a special block, not a function definition. It registers a per-frame callback. This is the core gameplay loop primitive.
- Time math is dimensional. `dt` is a `Duration`; `200 * dt` produces a length because `200` is interpreted as `pixels/second` in this context. (This is aspirational and may be relaxed; see `06-design-document.md`.)
- No semicolons, no braces, indentation-based blocks (Python / GDScript family). Easier for LLMs (fewer formatting tokens) and beginners (less syntax noise).

---

## Example 2 — Inventory and item modifiers

```twe
item Sword:
    damage: 10..15        # range — randomized per-instance roll
    crit_chance: 5%
    weight: 3kg
    rarity: common
    on_hit(target):
        target.damage(self.damage.roll())

item FlameBlade extends Sword:
    damage: 20..30
    rarity: rare
    on_hit(target):
        super.on_hit(target)
        target.ignite(duration: 3s, dps: 2)

modifier Sharpened:
    damage: +20%
    crit_chance: +2%

inventory player:
    capacity: 20kg
    slots:
        weapon: Sword
        offhand?: Shield     # ? = optional slot

# usage
sword = FlameBlade()
sword.apply(Sharpened)
player.equip(sword, slot: weapon)
```

**Demonstrates:** the flagship example for the 2D RPG-systematic pillar. Items, modifiers, inventories — the data heart of any progression-driven game.

**Implied decisions:**

- `item`, `modifier`, `inventory` are first-class declarative blocks. They compile to entity / component / system primitives under the hood, but the user never sees that.
- Range literals (`10..15`) and percentage literals (`5%`) are typed values with semantics, not just syntactic sugar. `range.roll()` samples it; percent arithmetic is well-defined.
- Dimensional units (`3kg`, `3s`) — concrete physical types prevent the classic bug of "is this milliseconds or seconds?" The compiler enforces unit compatibility.
- `extends` for inheritance, `super` for parent behavior. **One** inheritance model — single inheritance, no traits / mixins / interfaces in v0.1.
- Modifiers are first-class. `+20%`, `+2%` declare *deltas* on a stat block; the runtime composes them. This is exactly how Path of Exile, Diablo, and Borderlands all work internally.
- Methods declared inside `item Sword:` are scoped to that item — no `function Sword:on_hit(target)` ceremony.
- `?` for optional fields. If a slot is optional, the type system knows.
- Keyword arguments (`duration: 3s, dps: 2`) are first-class.

---

## Example 3 — Branching dialogue with timing

```twe
dialogue MeetMerchant:
    actor merchant = scene.npc("merchant")

    merchant.face(player)
    say merchant: "A traveler! Come closer..."
    wait 0.5s

    choice:
        "Show me your wares.":
            merchant.open_shop()
        "Who are you?":
            say merchant: "Just a humble merchant."
            wait 1s
            say merchant: "...or am I?"
            merchant.glow(color.purple, 2s)
        "[Leave]":
            merchant.face_away()
            return

    say merchant: "Safe travels."
```

**Demonstrates:** the coroutine showcase. If this code reads naturally, ~80% of why scripting RPGs is painful today is solved.

**Implied decisions:**

- `dialogue` is a coroutine block. Lines containing `wait` or `say` (which has implicit wait-for-input) suspend without blocking the engine.
- `say <actor>: "..."` is a built-in form, not a function call. Dialogue is *the* most common thing to write in an RPG; it deserves syntax.
- `choice:` block with indented options compiles to a UI prompt that returns the chosen branch.
- `wait <duration>` is a statement. Like `await sleep(0.5)` but with no async coloring — coroutines are transparent.
- `return` inside a `dialogue` block exits the dialogue, not the enclosing function. Dialogue blocks have their own control-flow scope.
- `scene.npc("merchant")` — current scene is ambient context; no import needed.

---

## Example 4 — NPC with state-machine AI

```twe
ai Goblin:
    initial: idle
    awareness: 8m
    speed: 3 m/s

    state idle:
        play_animation("idle")
        on player.within(awareness): -> alert

    state alert:
        play_animation("alert")
        wait 0.5s
        -> chase

    state chase:
        move_toward(player, speed)
        on player.within(2m): -> attack
        on player.beyond(awareness * 1.5): -> idle
        on hp < 20%: -> flee

    state attack:
        face(player)
        every 1s:
            player.damage(roll(5..8))
            play_animation("swing")
        on player.beyond(2m): -> chase

    state flee:
        move_away_from(player, speed * 1.5)
        wait 5s
        -> idle
```

**Demonstrates:** behavior-over-time without state machine boilerplate.

**Implied decisions:**

- `state <name>:` is a labeled block. `-> <name>` transitions. `on <event>` registers a state-scoped event handler that's only active while in this state.
- States automatically deregister their handlers on exit. A huge bug class is eliminated by construction.
- `every <duration>:` is an in-state recurring action that compiles to a coroutine with `wait`.
- `hp < 20%` evaluates `hp` against the entity's max — `%` on a stat is implicitly relative. (This requires type-aware operator semantics; see design doc.)
- Speed has units (`m/s`), distance has units (`8m`). 2D/3D agnosticism falls out — `m` is just a length.
- `->` is the only special arrow. No `=>`, no `<-`. Minimal symbol vocabulary.

---

## Example 5 — Procedural fire effect (the differentiator)

```twe
visual Fire:
    size: (64, 96)

    pixel(uv, time) -> color:
        # uv is normalized (0..1, 0..1); origin top-left
        flame_shape = smoothstep(0.5 - uv.y * 0.4, 0.5, uv.x)
                    * smoothstep(0.5 + uv.y * 0.4, 0.5, 1 - uv.x)

        n = noise(uv * 4 + (0, -time * 3))
        intensity = flame_shape * (n + 1 - uv.y)

        return mix(color.transparent,
                   mix(color.yellow, color.red, uv.y),
                   intensity)

# usage
torch = entity.at(100, 200)
torch.attach(Fire)
```

**Demonstrates:** the headline feature. No GLSL file. No texture file. A fire effect described in pure Twe that runs on the GPU.

**Implied decisions:**

- `visual` is a special block that compiles to a fragment shader (GLSL or WGSL depending on backend). The `pixel(uv, time) -> color` signature is fixed.
- The Twe subset inside `visual` is restricted: no allocations, no loops without compile-time bounds, no calling host code. This is the price of "pure code visuals" and the LLM tooling must understand the restriction.
- `color` is a built-in type with named constants (`color.red`, `color.transparent`).
- Built-in stdlib functions: `noise`, `smoothstep`, `mix`. They work both in CPU code and in `visual` blocks (with the same semantics where defined).
- Vectors support arithmetic (`uv * 4 + (0, -time * 3)`) and swizzling (`uv.x`, `uv.y`). Native to the language, not via operator overloading on a third-party class.

---

## Example 6 — Particle burst

```twe
particles ExplosionBurst:
    count: 50
    lifetime: 0.6s
    emit_pattern: radial

    on_spawn(p):
        p.velocity = random.in_circle(radius: 200..400)
        p.color = random.choice([color.orange, color.red, color.yellow])
        p.size = 4..8

    on_update(p, dt):
        p.velocity *= 0.95           # drag
        p.color.alpha = (1 - p.age_ratio) ^ 2   # fade out
        p.size *= 1 - 0.5 * dt       # shrink

# usage
on enemy.death(e):
    spawn ExplosionBurst at e.pos
```

**Demonstrates:** physics + visuals at a higher level than Example 5.

**Implied decisions:**

- `particles` is its own block type. The block declares the *system*; the runtime instantiates particles.
- `p.age_ratio` is an implicit field provided by the runtime — `0` at spawn, `1` at death.
- `random.in_circle(radius: ...)` uses keyword arguments. Twe supports keyword args throughout.
- `spawn <thing> at <position>` is a built-in form.
- `on enemy.death(e):` is a global event handler that listens for any entity matching the `enemy` archetype. This implies a pub/sub layer in the runtime — which is also how ECS query systems work (see `03-runtime.md`).

---

## Example 7 — Save and load

```twe
save SaveSlot:
    version: 2
    player:
        pos: vector
        hp: int
        inventory: list of Item
    world:
        seed: int
        time_of_day: time

    migrate from version 1:
        # version 1 had `health` instead of `hp`
        player.hp = old.player.health

# usage
save_to("slot1.save", as: SaveSlot {
    player: { pos: hero.pos, hp: hero.hp, inventory: hero.inv },
    world: { seed: world.seed, time_of_day: world.time }
})

state = load_from("slot1.save", as: SaveSlot)
hero.pos = state.player.pos
```

**Demonstrates:** the most-forgotten part of game scripting — versioned save schemas. Most engines force manual `(de)serialize` and version-handle. Twe makes save schemas a language construct.

**Implied decisions:**

- Save formats are *declared schemas*, not ad-hoc serialization.
- Schema migrations are first-class. `migrate from version <n>:` runs automatically on load. This solves a major painpoint of long-development indie games (saves from old builds breaking).
- Type annotations exist (`pos: vector`, `hp: int`) but are opt-in and only mandatory in schemas. **Gradual typing.** See `02-type-system.md`.
- Block literals (`SaveSlot { ... }`) are how typed values are constructed.

---

## Example 8 — 3D camera follow with smoothing

```twe
scene Forest3D:
    terrain = load_mesh("forest.glb")
    hero = spawn Hero at (0, 0, 0)

    camera.mode: third_person
    camera.target: hero
    camera.offset: (0, 4, -8)        # behind and above
    camera.smoothing: 0.15s

    on update(dt):
        if key.w: hero.move_forward(5 * dt)
        if key.a: hero.turn(-90 deg/s * dt)
        if key.d: hero.turn(90 deg/s * dt)
```

**Demonstrates:** the 2D-to-3D bridge. The language is unchanged from Example 1; only the stdlib expands.

**Implied decisions:**

- Same `on update(dt)` from Example 1. Vectors are now 3-component because `(0, 4, -8)` has three values. The runtime infers dimension.
- `camera` is an ambient scene singleton.
- `third_person` is an enum value, accessed without prefix because the type is known from the assignment target.
- Angular units (`90 deg/s`). `deg` and `rad` interconvert; `deg/s` is angular velocity.
- `scene Forest3D:` is a block. Loading a scene is `enter Forest3D` — the runtime handles teardown of the previous scene.
- `load_mesh` returns a 3D asset; `load` (from Example 1) returns a sprite. Polymorphism is by file extension or explicit function — keeping Twe simple, not magical.

---

## Example 9 — Tilemap with collision

```twe
tilemap Dungeon:
    tile_size: 16px
    tiles:
        ".": floor       (walkable)
        "#": wall        (solid)
        "~": water       (slow: 50%)
        "X": exit        (trigger)

    layout: """
        ################
        #..............#
        #...~~..........#
        #....~~~........#
        #.........X.....#
        ################
    """

# usage
enter Dungeon
hero = spawn Hero at Dungeon.spawn_point
on hero.enter(tile: exit):
    enter NextLevel
```

**Demonstrates:** 2D world building. Tilemaps are so common in 2D games that they deserve language-level support.

**Implied decisions:**

- Tilemaps are first-class.
- Triple-quoted multiline strings (Python-style).
- `(walkable)`, `(solid)`, `(slow: 50%)` are tile traits — properties the runtime understands. Collision falls out automatically; "slow" multiplies movement speed for entities on that tile.
- `on hero.enter(tile: exit):` is a typed event handler — listens for `hero` entering any tile of type `exit`.

---

## Example 10 — A small boss fight (the integration test)

```twe
boss SlimeKing extends Enemy:
    hp: 500
    damage: 20
    sprite: load("slime_king.png")
    visual aura: AuraGlow(color: green)   # references a `visual` block elsewhere

    state phase1:
        every 2s: spawn SmallSlime at self.pos + random.in_circle(50)
        on hp < 60%: -> phase2

    state phase2:
        play_animation("enraged")
        damage: 30           # stat override during phase2
        every 1s:
            telegraph(zone: circle(200), warning: 0.5s) then
                if player.in_zone: player.damage(self.damage)
        on hp < 25%: -> phase3

    state phase3:
        speed: 0   # rooted
        every 3s:
            shoot Projectile {
                count: 12,
                pattern: ring,
                speed: 300,
                on_hit(target): target.poison(5s)
            }
        on hp <= 0: -> defeated

    state defeated:
        play_animation("dying")
        spawn ExplosionBurst at self.pos
        wait 1s
        despawn self
        player.grant_loot(SlimeCrown, GoldPile(500..1000))
```

**Demonstrates:** the integration test. Combines pillars 1 (data), 2 (behavior over time), and 3 (visuals) in a recognizable RPG game pattern.

**Implied decisions:**

- Stats are mutable per-state (`damage: 30` inside `phase2`). The runtime restores the base value on state exit.
- `telegraph(...) then ...` is a sequencing primitive. `then` waits for the previous action to complete before running the next. Different from `;`.
- Block-literal arguments (`shoot Projectile { count: 12, ... }`). Inline configuration for spawned entities.
- `if player.in_zone:` — standard `if` works on entity properties seamlessly.
- `despawn self` is a built-in primitive.

---

## What the ten examples have collectively decided

### Lexical level

- Indentation-based blocks (no braces).
- No semicolons.
- Comments start with `#`.
- Triple-quoted multiline strings.
- Dimensional unit literals: `3kg`, `200ms`, `90deg`, `5m/s`, `60deg/s`.
- Range literals: `10..15`.
- Percent literals: `5%`.
- String literals with double quotes for single-line, `"""..."""` for multi-line.

### Type system (preview — see `02-type-system.md`)

- Gradual typing: opt-in, inferred where possible, required in schemas.
- Native types: `int`, `float`, `bool`, `string`, `vector` (2D or 3D), `color`, `range`, `duration`, `length`, `mass`, `angle`.
- Tuples auto-coerce to vectors.
- Block literals (`Item { ... }`) for typed value construction.

### Control flow

- Standard: `if` / `else` / `for` / `while` / `return`.
- Game-specific: `wait <duration>`, `every <duration>:`, `on <event>:`, `state <name>:`, `-> <state>`, `then` for sequencing.

### Declarative blocks

The bold move. These are top-level forms that the language treats as data + behavior declarations:

- `entity` — base form
- `item`, `modifier`, `inventory` — RPG data
- `dialogue`, `choice` — narrative
- `ai`, `state` — behavior
- `visual`, `particles` — graphics
- `tilemap`, `scene` — world
- `save` — persistence
- `boss`, `enemy`, `npc` — game-specific subtypes (probably stdlib, not core)

**Risk:** that's roughly 14 special forms. Too many for v0.1.

**Mitigation for v0.1:** ship six core forms (`entity`, `state`, `visual`, `particles`, `scene`, `dialogue`). Let the rest emerge as patterns built on top. Promote them to keywords only after they prove themselves in real games.

### Runtime model

- ECS-flavored under the hood. Entities, components, systems — but the user sees declarative blocks.
- Coroutines / fibers are transparent. No `async` / `await` distinction.
- Events are the cross-cutting glue. `on <event>:` handlers compile to a pub/sub layer that integrates with ECS queries.

---

## Examples deliberately *not* included (and why)

- **Multiplayer / networking.** Out of scope for v0.1. Will inform v0.2 design.
- **Custom UI / HUD.** Important but follows from `entity` + `visual` + event handlers.
- **Audio.** Stdlib concern, not a language design driver.
- **Modding / sandboxing.** Important for v1.0. Follows from a clean module system.
- **Asset pipeline / hot reload.** Tooling concern, addressed in `05-roadmap.md`.

These domains will be pulled in once v0.1 is real and the friction is concrete.

---

## What to do when the design is in doubt

Return to this document. If a feature isn't required by any of the ten examples, it probably shouldn't be in v0.1. If a syntactic decision makes one of these examples awkward, that's a signal the decision is wrong. The examples are the spec.
