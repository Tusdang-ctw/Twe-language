# Doc 06 — Twe Language Design Document (Formal Spec, v0.1-draft)

> The formal specification for Twe v0.1. Subject to revision until the implementation reaches Phase 3 of the roadmap.
>
> This document defines what Twe *is*. The other documents define why and how. When this document and another disagree, this one wins for v0.1; another iteration of the design loop will follow.

**Status:** draft. **Version:** 0.1.0-pre. **Last revised:** 2026-04-27.

---

## Table of contents

1. [Principles](#1-principles)
2. [Lexical structure](#2-lexical-structure)
3. [Grammar (EBNF)](#3-grammar-ebnf)
4. [Semantics](#4-semantics)
5. [Type system reference](#5-type-system-reference)
6. [Built-in types](#6-built-in-types)
7. [Standard library overview](#7-standard-library-overview)
8. [Error model](#8-error-model)
9. [Modules and scoping](#9-modules-and-scoping)
10. [Reserved words and operators](#10-reserved-words-and-operators)
11. [Future work](#11-future-work)

---

## 1. Principles

In strict priority order:

**P1. Game concepts are first-class.** `entity`, `state`, `visual`, `dialogue`, `particles`, `scene` are language constructs, not library calls.

**P2. One obvious way per concept.** Single inheritance, one method-call syntax, one looping construct family, one OOP idiom. Regularity benefits humans and LLMs equally.

**P3. No silent footguns.** Zero-indexed arrays, only `false` is falsy, dimensional units enforced, errors that suggest fixes.

**P4. AI-legible by design.** Predictable grammar (LL(1)-ish), structured diagnostics, round-trippable AST, no context-sensitive parsing.

**P5. Engine-native.** Twe's runtime is the engine's runtime. Engine objects are first-class Twe values, not opaque userdata.

When these principles conflict, lower-numbered ones win.

---

## 2. Lexical structure

### 2.1 Source encoding

Twe source files are UTF-8 encoded. Identifiers and string contents may contain any valid Unicode scalar value; keywords and operators are restricted to ASCII.

File extension: `.twe`.

### 2.2 Whitespace and indentation

Twe uses indentation to delimit blocks (Python-family). Indentation must be consistent within a block; mixing tabs and spaces in one file is a parse error.

The recommended indentation is **four spaces**. The compiler accepts any consistent unit.

A logical line ends at a newline that is not inside a parenthesized expression or a triple-quoted string. Backslash at end of line is **not** a line continuation; use parentheses for multi-line expressions.

### 2.3 Comments

```twe
# single-line comment

#: doc comment — attaches to the next declaration
#: can span multiple lines if they all start with #:

#-
  block comment for temporarily disabling code
  may not be nested
-#
```

### 2.4 Identifiers

```
identifier := letter (letter | digit | "_")*
letter     := "A".."Z" | "a".."z" | "_"
digit      := "0".."9"
```

Identifiers are case-sensitive. By convention:

- `snake_case` for variables, functions, fields.
- `PascalCase` for types, declarative-block names (`Sword`, `MeetMerchant`).
- `SCREAMING_SNAKE` for compile-time constants only.

### 2.5 Literals

#### 2.5.1 Numeric literals

```
int_literal    := digit+
float_literal  := digit+ "." digit+ ([eE] [+-]? digit+)?
hex_literal    := "0x" hex_digit+
binary_literal := "0b" ("0" | "1")+
```

Examples: `42`, `3.14`, `1.5e-3`, `0xFF`, `0b1010`.

Underscores are allowed as digit separators: `1_000_000`, `0xFF_FF`.

#### 2.5.2 String literals

```
string_literal      := '"' (escape | char)* '"'
multiline_literal   := '"""' any* '"""'
interp_literal      := '"' (escape | char | "{" expr "}")* '"'
```

Examples: `"hello"`, `"line1\nline2"`, `"hi {name}"`, `"""multi\nline"""`.

Escape sequences: `\n \r \t \\ \" \{ \u{1F600}` (Unicode escape).

#### 2.5.3 Boolean and nil literals

`true`, `false`, `nil`.

#### 2.5.4 Range literals

```
range_literal := expr ".." expr        # inclusive
              | expr "..<" expr         # exclusive end
```

Examples: `10..15` (inclusive 10–15), `0..<count` (exclusive end). Type is `range of T` where `T` is the type of the bounds.

#### 2.5.5 Percent literals

```
percent_literal := number "%"
```

Examples: `5%`, `25.5%`. Type is `percent`. `5%` is *not* equal to `0.05`; conversion is explicit via `.as_fraction()`.

#### 2.5.6 Unit literals

Twe has built-in dimensional units. The unit is part of the literal token.

```
unit_literal := number unit_suffix

unit_suffix :=
  | "s" | "ms" | "min" | "h"          # duration
  | "m" | "cm" | "mm" | "km" | "px"   # length
  | "kg" | "g" | "mg"                 # mass
  | "deg" | "rad"                     # angle
  | unit_suffix "/" unit_suffix        # compound (e.g., m/s)
```

Examples: `0.5s`, `200ms`, `5m`, `30cm`, `100px`, `90deg`, `5 m/s`, `60 deg/s`.

The space before the unit is optional but allowed for compound units (`5 m/s` and `5m/s` both parse). Compound units are composed at the type level (see §6).

#### 2.5.7 Tuple literals

```
tuple_literal := "(" expr ("," expr)* ","? ")"
```

Examples: `(1, 2)`, `(x, y, z)`, `(0, 4, -8)`.

A 1-element tuple is written `(x,)` (trailing comma) to disambiguate from grouping.

### 2.6 Keywords

See §10 for the full list.

---

## 3. Grammar (EBNF)

This grammar is informal EBNF. The parser is hand-written recursive descent; this serves as the contract.

### 3.1 Top level

```
program := statement*

statement :=
  | var_decl
  | function_decl
  | declarative_block
  | event_handler
  | type_alias
  | import_decl
  | expression
  | assignment
  | control_flow
  | "return" expr?
  | "yield" expr?
```

### 3.2 Declarations

```
var_decl       := ("let" | "var") identifier (":" type)? "=" expr
                 # `let` is immutable; `var` is mutable

function_decl  := "function" identifier "(" param_list? ")" ("->" type)? ":" block

param_list     := param ("," param)*
param          := identifier (":" type)? ("=" expr)?

type_alias     := "type" identifier "=" type

import_decl    := "import" module_path ("as" identifier)?
                | "from" module_path "import" identifier ("," identifier)*
module_path    := identifier ("." identifier)*
```

### 3.3 Declarative blocks

These are the heart of Twe. A declarative block is a top-level form whose name and contents the runtime treats as data + behavior.

```
declarative_block :=
  block_keyword identifier ("extends" identifier)? ":" indented_block

block_keyword :=
  | "entity" | "item" | "modifier" | "inventory"
  | "ai" | "state"
  | "visual" | "particles"
  | "scene" | "tilemap"
  | "dialogue"
  | "save"

indented_block := INDENT (field | method | nested_block)* DEDENT

field          := identifier ":" type? "=" expr
                | identifier ":" type
                | identifier "?:" type            # optional field

method         := "function"? identifier "(" param_list? ")" ":" block
                | event_handler

nested_block   := "state" identifier ":" indented_block
                | "choice" ":" indented_block
                | "every" duration ":" block
```

V0.1 ships with **six** core block keywords: `entity`, `state`, `visual`, `particles`, `scene`, `dialogue`. The other forms (`item`, `inventory`, `ai`, `tilemap`, `save`) are stdlib-defined patterns that desugar to `entity` plus convention. They may be promoted to keywords in v0.2 once their semantics are stable.

### 3.4 Event handlers

```
event_handler := "on" event_pattern ":" block

event_pattern := identifier ("." identifier)* ("(" param_list? ")")?
              | predicate_expression

predicate_expression := expr "<" expr | expr ">" expr | ...
                       # e.g., "on hp < 20%:"
```

### 3.5 Expressions

```
expr := assignment_expr

assignment_expr := unary_expr (assign_op assignment_expr)?
assign_op       := "=" | "+=" | "-=" | "*=" | "/="

binary_expr := primary (binary_op primary)*
binary_op   := "+" | "-" | "*" | "/" | "%" | "^"
            | "==" | "!=" | "<" | ">" | "<=" | ">="
            | "and" | "or" | "not"
            | ".." | "..<"

# `and` / `or` are value-returning short-circuit (Python-like): they return
# one of their operands, not a strict Bool. `not` returns strict Bool.
# Combined with Principle 3 (only `false` is falsy), this means
# `count or default` returns `count` even when `count == 0` — the
# Lua/JS/Python footgun does not apply here. See
# docs/changes/2026-04-28-or-and-keep-value-returning.md.

unary_expr  := ("-" | "not") unary_expr | postfix_expr
postfix_expr := primary postfix*
postfix      := "." identifier
             | "(" arg_list? ")"
             | "[" expr "]"
             | "?"                     # optional unwrap, e.g., maybe_thing?

arg_list     := arg ("," arg)* ","?
arg          := expr                   # positional
             | identifier ":" expr     # keyword (must follow all positional)

primary :=
  | literal
  | identifier
  | "(" expr ")"
  | tuple_literal
  | list_literal
  | map_literal
  | block_literal
  | lambda
  | "self" | "super"

block_literal := identifier "{" field_init ("," field_init)* "}"
field_init    := identifier ":" expr

lambda := "fn" "(" param_list? ")" ("->" type)? ":" (block | expr)
```

### 3.6 Control flow

```
control_flow :=
  | if_expr
  | while_loop
  | for_loop
  | wait_stmt
  | every_block
  | match_expr
  | transition

if_expr  := "if" expr ":" block ("elif" expr ":" block)* ("else" ":" block)?
while_loop := "while" expr ":" block
for_loop := "for" identifier "in" expr ":" block
wait_stmt := "wait" expr             # expr must be Duration
every_block := "every" expr ":" block
match_expr := "match" expr ":" INDENT (match_arm)+ DEDENT
match_arm := pattern ":" block
transition := "->" identifier        # state transition; only legal inside `state`
```

### 3.7 Type expressions

```
type :=
  | identifier ("<" type ("," type)* ">")?
  | "(" type ("," type)* ")"            # tuple type
  | type "?"                              # optional
  | type "|" type                         # union
  | "list" "of" type
  | "map" "of" type "=>" type
  | "set" "of" type
  | "fn" "(" type ("," type)* ")" "->" type
```

---

## 4. Semantics

### 4.1 Evaluation model

Twe has eager evaluation. Expressions are evaluated left to right. Function arguments are evaluated before the call. Short-circuit operators (`and`, `or`) skip their right operand when the left determines the result.

### 4.2 Variables and scoping

- **`let`** declares an immutable binding.
- **`var`** declares a mutable binding.
- Scopes are block-scoped. A name introduced in a block goes out of scope at the end of that block.
- Shadowing within a block is a compile-time error. Shadowing across nested blocks is allowed.
- Top-level `let` and `var` are module-scoped.
- A `global` keyword exists for explicit globals (used sparingly, mainly for engine-provided ambient values like `time`, `key`, `scene`).

### 4.3 Functions

Functions are first-class values. They close over their enclosing scope. They have exactly one return value (which may be a tuple).

```twe
function add(a: int, b: int) -> int:
    return a + b

let inc = fn(x: int) -> int: x + 1   # lambda

let result = add(2, inc(3))   # 6
```

### 4.4 Method dispatch

Methods are functions declared inside a declarative block. They receive `self` implicitly.

```twe
entity Sword:
    damage: int = 10
    function attack(target: Entity):
        target.hp -= self.damage
```

There is no separate operator for method calls. `sword.attack(goblin)` works because `attack` is defined on `Sword`. There is no `:` vs `.` distinction.

### 4.5 Inheritance

Single inheritance via `extends`. A child block inherits all fields and methods of its parent. Methods can be overridden; `super.method(...)` calls the parent's version.

There are no abstract types, interfaces, or traits in v0.1.

### 4.6 Coroutines (fibers)

Twe has transparent coroutines. Any function that contains `wait`, `every`, or `yield` automatically runs as a fiber. There is no `async` / `await` distinction.

```twe
function play_intro():
    say "Welcome."
    wait 2s
    say "To Twe."
```

This function is a fiber. Calling it yields control back to the scheduler at each `wait` and `say` (which has implicit input-wait). The scheduler resumes the fiber when the wait completes.

Fibers cannot escape their declaring scope (no first-class fiber values in v0.1; this may relax in v0.2).

**v0.1 implementation status (Phase 5 task 2).** `wait <duration>` is implemented as a *direct* statement of a state's on-entry body in **both backends**:

- **Tree-walker:** the runtime stores a resume index + remaining seconds on the instance; `tick_scene` decrements the timer and resumes the body when it elapses.
- **Bytecode VM:** `OP_WAIT` pops a duration, saves the chunk + resume IP + remaining seconds on the `BcInstance`, then collapses the call frame (synthetic Nil return). `tick_scene` re-pushes the saved frame and continues dispatch from the saved IP once the timer elapses.

While suspended, the state's `every`-clocks and `on update(dt):` are paused — the state is "asleep" until the wait fires. Outstanding work (later Phase 5 sessions): function-body `wait`, fiber-backed `every` rewrite, `wait` inside `dialogue`. Using `wait` outside a state on-entry surfaces a clear error (compile-time on the bytecode VM, runtime on the tree-walker) pointing at the limitation.

### 4.7 Events

The `on <event>:` form registers an event handler. There are three event kinds:

1. **Frame events:** `on update(dt):`, `on render():`. Fire every frame.
2. **Predicate events:** `on hp < 20%:`. Fire when the predicate transitions from false to true.
3. **Named events:** `on enemy.death(e):`. Fire when something publishes that event.

Handlers are scoped:

- At the top level, they run for the entire program.
- Inside a `state` block, they run only while in that state and are deregistered on state exit.
- Inside an `entity` declaration, they run for the lifetime of that entity instance.

### 4.7a Every-clock catch-up (v0.1)

Each `every <duration>:` clock keeps a per-instance accumulator. When `tick_frame(dt)` runs, the runtime adds `dt` to every active accumulator and fires the clock body once for every full interval that has accumulated, capped at **8 fires per clock per frame**. If the cap is hit, the accumulator is reset to zero so the backlog doesn't compound. This means: (1) at the normal 60fps frame rate, every-clocks fire exactly as expected; (2) after a brief stall, a small backlog catches up; (3) after a long pause (debugger, alt-tab), the runtime drops the missed time rather than freezing for catch-up.

### 4.8 State machines

A `state` block inside an `ai` declaration (or any container that supports states) defines a state machine.

```twe
ai Goblin:
    initial: idle

    state idle:
        on player.within(8m): -> alert

    state alert:
        wait 0.5s
        -> chase
```

Semantics:

- Exactly one state is active at a time.
- Entering a state runs its body top-to-bottom.
- Event handlers (`on ...`) inside a state are active only while in that state.
- `every <duration>:` blocks are scheduled when entering, cancelled on exit.
- `-> <state>` transitions immediately. Code after the transition is dead.
- Statement-level `wait` inside a state suspends without leaving the state.
- `on <predicate>:` registers an edge-triggered handler scoped to the state. The runtime evaluates the predicate each frame and fires the body on the false → true transition. The body is *not* re-fired while the predicate stays true (Phase 5 task 4 — both backends ship this).

### 4.9 Visual blocks

A `visual` block compiles to a fragment shader.

```twe
visual MyEffect:
    pixel(uv, time) -> color:
        return color(uv.x, uv.y, 0, 1)
```

Restrictions inside `pixel`:

- No allocations.
- No calls to non-`visual`-safe functions (the stdlib marks each).
- Loops must have compile-time-known bounds.
- No I/O, no entity manipulation.

The compiler translates the `pixel` body to GLSL or WGSL depending on the runtime's GPU backend.

### 4.10 Particles

A `particles` block declares an emitter. The runtime instantiates particles up to `count`, runs `on_spawn(p)` once per particle, and `on_update(p, dt)` every frame.

Each particle has implicit fields: `pos`, `velocity`, `color`, `size`, `age` (in seconds), `age_ratio` (`age / lifetime`).

**v0.1 implementation status:** the declarative block is in. `count` (int, default 16) and `lifetime` (float seconds, default 1.0) are read at spawn time; `on_spawn(p)` and `on_update(p, dt)` are called per particle if defined. The runtime ages each particle (`age += dt`, `age_ratio = age / lifetime`) every frame and despawns the emitter when no particles are left. Default rendering draws each particle as a `draw_circle(p.pos, p.size, p.color)` — define `function render():` on the particles block to override. `emit_pattern` and keyword args are deferred until F1 (per the Phase 2 frustration list).

### 4.10a Dialogue runtime (Phase 5 task 3, partial)

`dialogue <Name>:` declares a sequenced block of statements. Calling `<Name>()` runs the body to completion. Inside a dialogue body, three statement forms are recognised (in addition to all normal statements):

```
dialogue MeetMerchant:
    say "A traveler approaches."           # bare narration
    say merchant: "Looking to trade?"      # actor-prefixed line
    choice:
        "Yes, show me your wares.":
            merchant.open_shop()
        "Just browsing.":
            ...
```

- `say <text-expr>` — prints the text to the runtime out buffer with a trailing newline.
- `say <actor-expr>: <text-expr>` — prints `Actor: text`. The actor expression's display: an `Instance` shows its class name (Wren-style); a string shows itself; anything else falls back to `display`.
- `choice:` — an indented list of `<label>:` branches. The runtime prints each label (numbered `[1]`, `[2]`, …) and runs the **first** branch's body. Real interactive selection is a follow-on once the UI surface (input modality, prompt rendering) is designed.
- `wait` inside a dialogue body raises a runtime error in v0.1 — per-dialogue suspension needs a separate scheduler from the per-state-instance one, planned for a Phase 5 follow-on.

The bytecode VM rejects `dialogue` / `say` / `choice` at compile time with a pointer at `--vm tree`; the tree-walker is the canonical execution path for this surface in v0.1.

### 4.11 Tilemaps

A `tilemap` block declares a tile-based map. The runtime handles rendering and collision based on tile traits (`solid`, `walkable`, `slow`, `trigger`).

### 4.12 Save schemas

A `save` block declares a versioned schema. `save_to(path, as: SaveSlot)` serializes; `load_from(path, as: SaveSlot)` deserializes and runs migrations.

### 4.13 Order of evaluation

For expressions: left-to-right.
For declarations at the top level: top-to-bottom.
For event handlers: by registration order, but the language makes no guarantee about determinism between handlers of the same event. **Don't depend on registration order between unrelated handlers.**

---

## 5. Type system reference

Twe v0.1 ships with non-strict mode only. See `02-type-system.md` for the full philosophy and the staged plan for strict and verified modes.

### 5.1 Inference rules

Types are inferred at:

- Variable declaration sites without annotations.
- Function returns without an explicit return type.
- Field initializers in declarative blocks.

The inference algorithm is Hindley-Milner with extensions for structural records, tagged unions, and dimensional units.

### 5.2 Subtyping

Twe has nominal subtyping for declarative blocks (an `entity Hero extends Sprite` is a subtype of `Sprite`) and structural subtyping for record types.

### 5.3 Optional types

`T?` means "either a `T` or `nil`." Indexing or member access on an optional requires explicit unwrap (`?.`, `if let`, or `match`).

```twe
let maybe_hero: Hero? = scene.find("hero")
if let hero = maybe_hero:
    hero.move(10, 0)
```

### 5.4 Union types

`A | B` is a tagged union. Match arms must exhaust the cases.

### 5.5 Dimensional checking

Operations on unit-typed values must be dimensionally compatible:

- `5m + 3m` is `8m`. Legal.
- `5m + 3s` is a type error.
- `5m / 1s` is `5 m/s`. Legal; the result type is the compound unit.
- `5m * 3` is `15m`. Legal; scalars are unit-less.

Conversion between compatible units is automatic at the type level (`30cm + 1m == 1.3m`).

---

## 6. Built-in types

| Type | Description | Literal example |
|------|-------------|-----------------|
| `int` | 64-bit signed integer | `42`, `0xFF` |
| `float` | 64-bit IEEE 754 | `3.14`, `1e-3` |
| `bool` | `true` or `false` | `true` |
| `string` | UTF-8 string | `"hello"` |
| `nil` | The single nil value | `nil` |
| `vector` | 2D or 3D vector of float | `(1, 2)`, `(0, 1, 0)` |
| `color` | RGBA color, components `0..1` float | `color.red`, `color(1, 0, 0)` |
| `range of T` | Numeric range | `10..15`, `0..<count` |
| `percent` | Percentage | `5%` |
| `duration` | Time | `0.5s`, `200ms` |
| `length` | Distance | `5m`, `100px` |
| `mass` | Mass | `3kg` |
| `angle` | Angle | `90deg`, `pi rad` |
| `velocity` | Length over time | `5 m/s` |
| `array of T` / `T[]` | Dynamic 0-indexed array | `[1, 2, 3]` |
| `map of K => V` | Hash map | `{ "a": 1, "b": 2 }` |
| `set of T` | Hash set | `set[1, 2, 3]` |
| `T?` | Optional T | (no literal; produced by computation) |
| `A \| B` | Tagged union | (no literal; produced by computation) |

---

## 7. Standard library reference

Every function below is available in `twec play` (the 2D runtime) unless marked **3D** or **visual**. Drawing primitives additionally require a render context (`on render():` block) — calling them outside render is a runtime error.

### 7.1 Core

```twe
print("hello {name}")        # interpolated string to stdout
assert(hp >= 0, "negative hp")  # panics with message when false
```

`print` accepts any value; interpolation `{expr}` converts to string automatically.

The `time` module exposes the simulation clock:

```twe
time.dt              # seconds since the last tick — equals time.physics_dt
                     #  inside `on update(dt)`. 0.0 before the first tick.
time.physics_dt      # constant 1/60s — the fixed-timestep simulation rate
                     #  the engine guarantees. Read at top level to size
                     #  velocity-per-step state independently of dt being 0.0
                     #  before any frame has run.
```

**Phase 29 session 1:** the play loop drives ticks at the fixed `time.physics_dt` rate using a Glenn Fiedler accumulator. On display refresh rates above 60 Hz, some renders skip a tick; below 60 Hz, some renders run two. Either way, `dt` inside `on update(dt)` is constant — required for replay determinism and lockstep multiplayer.

The `gc` module configures the tracing-GC budget and exposes observability:

```twe
gc.budget_ms(2.0)        # cap per-safepoint sweep work at 2ms (default 2ms).
                         #  Lower → smoother frame times, slower reclamation.
                         #  Pass a tiny number like 0.5 for tight pacing budgets.
gc.last_collect_ms()     # wall-clock cost of the last completed sweep cycle,
                         #  in ms. Aggregates across however many incremental
                         #  steps the cycle took.
gc.bytes_alive()         # live heap bytes since the last sweep cycle finished.
```

**Phase 29 session 2:** sweep is incremental. The play-loop safepoint runs the mark phase once per cycle, then sweeps up to `gc.budget_ms` worth of objects per call; the cursor persists across frames so a large heap is collected over multiple safepoints rather than stalling one. Allocations during an in-flight sweep are pre-marked so they survive the round. The mark phase itself is still stop-the-world; bounding mark requires tri-color which lands as a follow-on.

The `replay` module captures and replays input frame logs:

```twe
replay.record("session.log")   # start writing input ambients each frame
replay.play("session.log")     # start replaying — synthetic input
                               #  overrides real keyboard/mouse
replay.stop()                  # end recording or playback
replay.is_playing()            # bool — true while replaying
```

**Phase 29 session 4:** the play loop calls into the replay subsystem after `update_key_state` and before the fixed-step accumulator runs. Recording snapshots the `key`, `key_press`, `mouse`, `mouse_held`, and `mouse_press` ambients to a small text log; playback overrides those ambients from the log so the script sees identical input across runs. Format details and what's not recorded (gamepad, system time, RNG reseeding) live in the module-level docs.

### 7.2 Math

All functions available as `math.<name>` and as bare names inside `visual` blocks.

```twe
math.abs(-3)           # 3
math.sqrt(9.0)         # 3.0
math.floor(3.7)        # 3
math.ceil(3.1)         # 4
math.min(2, 5)         # 2
math.max(2, 5)         # 5
math.mod(-1, 4)        # 3 — Euclidean modulo. `%` is reserved for percent literals.
math.sin(math.pi)      # ~0
math.cos(0.0)          # 1.0
math.noise((x, y))     # deterministic 2D value noise, range [-1, 1]
math.smoothstep(0.0, 1.0, t)  # smooth 0→1 curve
math.mix(a, b, t)      # linear interpolation; works on numbers or same-shape tuples
math.clamp(v, lo, hi)  # clamp v to [lo, hi]
math.pi                # 3.14159…
```

### 7.3 Random

```twe
random.float()              # float in [0, 1)
random.int(0..<10)          # int in [0, 9]
random.in_circle(radius: 40.0)    # (x, y) tuple uniformly in a circle
random.choice(["a", "b", "c"])    # random element from a list
random.shuffle(deck)              # in-place Fisher-Yates; returns nil
random.seed(42)                   # re-seed the PRNG (testing / replays)
```

### 7.4 Color

Named constants: `color.red`, `color.green`, `color.blue`, `color.white`, `color.black`, `color.gray`, `color.yellow`, `color.cyan`, `color.magenta`, `color.orange`, `color.purple`.

```twe
color.from_hex("#ff8800")        # parse #rrggbb or #rrggbbaa
color.hsv(200.0, 0.8, 1.0)      # hue in degrees, saturation/value 0–1
color.lerp(color.red, color.blue, 0.5)        # perceptual blend
color.lerp_linear(color.red, color.blue, 0.5) # gamma-correct blend
color.to_linear(color.red)       # sRGB → linear
color.to_srgb(c)                 # linear → sRGB
```

### 7.5 Drawing  *(render context required)*

```twe
rect(at: (10, 20), size: (100, 50), color: color.red)
circle(at: (320, 240), radius: 30.0, color: color.blue)
circle_outline(at: (320, 240), radius: 30.0, thickness: 2.0, color: color.cyan)
line(from: (0, 0), to: (100, 100), width: 2.0, color: color.white)
text("Score: {score}", at: (10, 10), size: 24, color: color.white)
```

Sprites and fonts (assets must be loaded first — see §7.9):

```twe
sprite(handle, at: (x, y))
sprite(handle, at: (x, y), size: (w, h))   # scaled
sprite_frame(atlas, at: (x, y), frame: 3)  # atlas cell, native size
sprite_frame_at(atlas, at: (x, y), size: (w, h), frame: 3)
text_with_font("Hi", at: (x, y), size: 18, color: color.white, font: my_font)
```

### 7.6 Input

**Keyboard** — `key.*` is held (true while down); `key_press.*` is edge-triggered (true for one frame on press):

```twe
on update(dt):
    if key.w:          # held
        move_up()
    if key_press.space:   # one frame
        jump()
```

Dynamic name lookup (for rebindable controls):

```twe
if key_held(settings.get("keys.up")):   # string key name
    dy -= 1.0
if key_pressed(settings.get("keys.fire")):
    shoot()
```

**Mouse:**

```twe
let x = mouse.x
let y = mouse.y
if mouse_press.left:    # click this frame
    fire()
if mouse_held.left:     # held
    drag()
let scroll = mouse.wheel
```

**Gamepad:**

```twe
if gamepad.connected:
    let lx = gamepad_axis.lx   # [-1, 1]
    let ly = gamepad_axis.ly
    if gamepad.a:              # held
        jump()
    if gamepad_press.start:    # edge-triggered
        pause(true)
```

Available buttons: `a b x y lb rb lt rt start select dup ddown dleft dright`. Axes: `lx ly rx ry lt rt`.

### 7.7 Entities

```twe
spawn Slime at (100.0, 200.0)          # creates a new instance
despawn slime                           # removes it (fields still valid this frame)

for s in entities.of(Slime):           # iterate live instances
    let d = s.pos.x - player_x

let count = entities.count(Slime)      # faster than iterating just to count
```

Death hooks fire whenever any instance of a class is despawned:

```twe
on Slime.death(s):
    spawn Spark at (s.pos.x, s.pos.y)
```

### 7.7b 3D post-processing  *(`twec play3d` only)*

```twe
postfx.tonemap(true)              # ACES filmic curve (default on)
postfx.vignette(0.4)              # radial darkening, 0 (off) to 1 (full)
postfx.vignette_color(color.purple) # tint instead of darken (default black)
postfx.bloom(0.6)                 # inline 12-tap bloom intensity, 0 (off)
postfx.bloom_threshold(1.0)       # HDR luminance above which bloom kicks in
postfx.frustum_cull(true)         # skip culled draw calls (default on)
```

The HDR pipeline always runs; `postfx.tonemap(false)` switches the final pass from ACES to a straight clamp. `bloom` is a single-pass inline kernel — small radius (~12 pixels), no multi-tier downsample chain. Cascaded shadow maps (3 cascades, 2K each layer, PCF) ship under the existing `sun.shadow(true)` switch.

### 7.8 Camera

**2D** (`twec play`):

```twe
camera.pos = (player_x, player_y)  # world-coord at screen center
camera.zoom = 1.5                  # >1 zooms in
camera.follow((player_x, player_y), 0.1)  # exponential smooth
camera.shake(8.0, 0.3)            # amplitude px, duration s
camera.reset()                    # snap to defaults
```

**3D** (`twec play3d`): `camera.eye`, `camera.target`, `camera.up` are 3-tuples.

### 7.9 Assets

```twe
let img   = load("assets/hero.png")
let atlas = load_atlas("assets/walk.png", (4, 1))  # (cols, rows)
let font  = load_font("assets/pixel.ttf")
let sfx   = sound.load("assets/hit.wav")
```

All loads are eager path-checked, lazy decoded. Errors on missing files fail fast.

### 7.9b Tilemaps

```twe
# Build a tilemap from a string layout. Each character is one tile.
let map = tilemap(
    layout: "...\n###\n...",
    tile_size: 32,
    tiles: [
        (".", "sky", []),
        ("#", "wall", ["solid"]),
    ]
)

tilemap_render(map, at: (0, 0))                     # blit
tilemap_at(map, x, y)                               # tile name at pixel (x, y), or ""
tilemap_solid_at(map, x, y)                         # tile carries the `solid` trait
tilemap_solid_aabb(map, x, y, w, h)                 # any corner of the AABB is solid
tilemap_aabb_touches(map, x, y, w, h, "spike")      # any corner of the AABB is named
```

The `_aabb` helpers sample all four corners of the box; they fit the platformer "is the player AABB clipping a tile?" pattern without a userland 4-corner loop. They do not sweep — at very high velocity an AABB can tunnel through a 1-tile-thick wall in a single frame, so cap fall speed accordingly. (A future swept primitive is on the roadmap.)

### 7.10 Audio

```twe
sound.play(sfx)                     # play once
sound.play_at(sfx, (x, y))         # positional (attenuates with distance)
sound.volume(sfx, 0.5)             # set volume 0–1

# Phase 29 session 5: tick-accurate scheduling for rhythm games.
sound.now()                         # simulation time in seconds
sound.schedule(sfx, when, vol)      # queue a one-shot for time `when`
sound.scheduled_count()             # how many entries are queued
```

WAV and Ogg Vorbis supported.

`sound.now()` returns the simulation clock that `sound.schedule(...)` deadlines compare against. The clock advances by exactly `time.physics_dt` (1/60s) per fixed-step substep, so a sound queued for `t = sound.now() + 0.5` fires on the same tick across two runs of the same input — the foundation for rhythm-game determinism.

**Honest deferral:** the underlying audio backend (macroquad's quad-snd) is buffer-aligned, not sample-aligned. Scheduled sounds fire on the simulation tick that crosses their deadline, giving ±1/60s ≈ ±16.7ms accuracy. True sample-accurate scheduling would require a different audio crate (cpal + custom mixer) — captured as a follow-on phase.

### 7.11 Save / load

```twe
save_to("save.json", { wave: wave_index, score: kills })
let data = load_from("save.json")
let wave = data.wave
```

`save_to` serializes any Twe value to JSON. `load_from` returns a record. Path is relative to the project directory at runtime; inside a built `.exe` the save file lands in the OS data directory.

### 7.12 Settings (persistent config)

```twe
settings.set_default("keys.right", "right")   # only sets if absent
settings.try_load("game.save")                # silently ok if missing

settings.set("vol", 0.8)
let v = settings.get("vol")       # nil if not set
let ok = settings.has("vol")      # bool

settings.save("game.save")
settings.load("game.save")        # errors if missing — use try_load for optional
```

### 7.13 UI widgets  *(render context required)*

All widgets are immediate-mode: they draw AND hit-test in the same call.

```twe
if button(at: (160, 200), size: (320, 50), label: "Start"):
    -> playing

label(at: (10, 10), size: (200, 30), text: "Score: {score}")
progress_bar(at: (10, 40), size: (200, 16), value: hp / max_hp)

var vol = slider(at: (10, 80), size: (200, 20), value: vol, min: 0.0, max: 1.0)
var muted = checkbox(at: (10, 110), size: (20, 20), value: muted)
var lang_idx = dropdown(at: (10, 140), size: (200, 30), options: ["EN","FR","JP"], selected: lang_idx)
var name = text_input(at: (10, 180), size: (200, 30), value: name)
var binding = key_input(at: (10, 220), size: (200, 30), value: binding)
```

**Layout helpers** — return `{ at, size }` so you can pipe into widgets:

```twe
let slot = grid(at: (20, 100), size: (600, 400), cols: 3, rows: 2, index: i, gap: 8)
if button(at: slot.at, size: slot.size, label: items[i]):
    select(i)
```

Also: `stack(...)`, `flex(...)`, `panel(at:, size:)`, `scroll(at:, size:, content_height:)`.

### 7.14 Pause

```twe
pause(true)          # halts every-clocks + entity ticks; render stays live
pause(false)
let p = is_paused()

auto_pause_when_idle(30.0)   # pause after 30s with no input
auto_pause_on_blur(true)     # pause when window loses focus (Windows; macOS/Linux stubbed)
```

### 7.15 Localization

```twe
lang.load("en", "assets/en.json")   # { "start": "Start Game", ... }
lang.load("fr", "assets/fr.json")
lang.set_locale("fr")

text(lang.t("start"), at: (200, 200), size: 24, color: color.white)
text(lang.tf("score", [kills]), at: (10, 10), size: 18, color: color.white)
```

`lang.t(key)` returns the key itself if the locale is missing the entry.

### 7.16 OS / clipboard

```twe
let text = os.clipboard.read()   # empty string if unavailable
os.clipboard.write("copied!")
```

### 7.17 Screenshot

```twe
screenshot("screenshot.png")   # saves current frame; also bound to F12 in twec play
```

### 7.18 Steam  *(optional — requires `--features steam` build)*

Available when the game is launched via Steam (Steam client running + `steam_appid.txt` present). All calls are no-ops in non-Steam builds or when Steam is not running.

```twe
achievement.unlock("FIRST_KILL")     # unlocks a Steam achievement by API name
stat.set("KILLS_TOTAL", kills)       # set an integer or float stat
stat.get("KILLS_TOTAL")              # returns the current value (0 if unset)
cloud.save("slot1.json", data_str)   # write to Steam Cloud (string payload)
cloud.load("slot1.json")             # returns string or nil if not found
```

Stats are committed to Steam servers automatically on clean exit. Call `stat.commit()` to flush mid-session.

---

## 8. Error model

### 8.1 Diagnostic categories

- **Syntax errors:** the source is not a valid Twe program. Reported at parse time.
- **Type errors:** in strict / verified modes only; in non-strict, type-related issues that *can* be proven to fail are reported, others are deferred.
- **Runtime errors:** raised during execution. Include nil-access, division by zero, index out of bounds, dimensional mismatch caught at runtime.

### 8.2 Diagnostic structure

Every diagnostic has:

- `kind` — one of the above categories.
- `severity` — `error`, `warning`, `info`, `hint`.
- `span` — file, line, column, length.
- `message` — human-readable.
- `suggested_fix` — optional structured fix the user (or LLM) can apply.

In verified mode, all diagnostics emit as JSON. See `02-type-system.md`.

### 8.3 Error handling

Twe uses **return-typed errors** for recoverable failures and **panics** for unrecoverable ones.

```twe
function read_config() -> Config | Error:
    let raw = io.read_file("config.json")
    match raw:
        Ok(text): return parse_config(text)
        Err(e):   return Error(e)

# panics terminate the current fiber:
assert(hp >= 0, "hp went negative — that's not supposed to happen")
```

Twe does **not** have try/catch in v0.1. The `match` form is the only way to handle a `T | Error`. Reasoning: try/catch hides control flow; explicit handling is clearer for both humans and LLMs.

---

## 9. Modules and scoping

### 9.1 File = module

Each `.twe` file is a module. The module's name is its filename (without extension). All top-level declarations are exported by default; prefix with `_` to mark as private (`function _internal_helper(...): ...`).

### 9.2 Imports

```twe
import math
import scene as s
from inventory import Sword, Shield
```

Imported names are resolved at compile time. There is no dynamic import in v0.1.

### 9.3 Module path resolution

Module paths resolve against:

1. The current project root (where `twe.toml` lives).
2. The standard library.
3. Project dependencies (in `twe.toml`).

A package manager (`twec add <pkg>`) is deferred to v0.2.

---

## 10. Reserved words and operators

### 10.1 Keywords

Reserved (cannot be used as identifiers):

```
and       elif      from      let       on        super     while
ai        else      function  list      or        then      yield
as        entity    global    map       particles tilemap
break     every     if        match     return    type
choice    extends   import    modifier  save      var
continue  false     in        nil       scene     visual
dialogue  fn        inventory not       self      wait
do        for       item      of        set       state
true
```

Total: roughly 50 keywords. This is at the high end for a small language. Some (like `do`, `then`) may be cut after Phase 2 reveals which are unused.

### 10.2 Operators

Arithmetic: `+ - * / % ^`
Comparison: `== != < > <= >=`
Logical: `and or not`
Range: `.. ..<`
Assignment: `= += -= *= /=`
Member / call: `. ( ) [ ]`
Type: `:` (annotation) `?` (optional) `|` (union)
Block: `:` (block start) `->` (transition / function return type)
Other: `...` (rest), `?.` (optional chaining), `=>` (map literal)

### 10.3 Reserved punctuation

`#` (comment), `"` (string), `'` (reserved for future use, e.g. character literals), `` ` `` (reserved), `@` (reserved for attributes / decorators in v0.2+).

---

## 11. Future work

Items deliberately deferred from v0.1, in rough priority order for later versions:

| Feature | Target version | Notes |
|---|---|---|
| Strict type checking mode | v0.2 | See `02-type-system.md` |
| Verified mode (LLM JSON diagnostics) | v0.3 | The differentiator |
| User-defined generics | v0.2 | Built-in collections only in v0.1 |
| Native code generation | v0.3 | Luau-style bytecode → machine code |
| Module / package manager | v0.2 | `twec add <pkg>` |
| Multi-threading / worker tasks | v0.4 | Single-threaded VM in v0.1 |
| Sandboxing for UGC | v1.0 | Roblox-style |
| Determinism for netcode | v1.0 | Hard guarantees |
| Decorators / attributes (`@deprecated`, `@inline`) | v0.2 | `@` is reserved |
| Macros / metaprogramming | uncertain | Possibly never |
| First-class fiber values | v0.2 | Currently scoped to declaration |
| Pattern matching beyond unions | v0.2 | Match guards, deep patterns |
| `try` / `catch` | uncertain | `T | Error` matching is sufficient for now |
| Reflection / introspection API | v0.3 | Useful for tooling |

---

## Appendix A — Worked example: parsing Example 2

This appendix shows how the parser handles Example 2 from `01-examples.md`, top to bottom. It exists to make the grammar concrete.

Source (excerpt):

```twe
item Sword:
    damage: 10..15
    crit_chance: 5%
    weight: 3kg
    rarity: common
    on_hit(target):
        target.damage(self.damage.roll())
```

Token stream (partial):

```
KEYWORD("item")  IDENT("Sword")  COLON  NEWLINE
INDENT
IDENT("damage")  COLON  RANGE_LIT(10, 15)  NEWLINE
IDENT("crit_chance")  COLON  PERCENT_LIT(5)  NEWLINE
IDENT("weight")  COLON  UNIT_LIT(3, "kg")  NEWLINE
IDENT("rarity")  COLON  IDENT("common")  NEWLINE
IDENT("on_hit")  LPAREN  IDENT("target")  RPAREN  COLON  NEWLINE
INDENT
IDENT("target")  DOT  IDENT("damage")  LPAREN
  KEYWORD("self")  DOT  IDENT("damage")  DOT  IDENT("roll")  LPAREN  RPAREN
RPAREN  NEWLINE
DEDENT  DEDENT
```

Parsed AST (sketch):

```
DeclarativeBlock(
  kind: "item",
  name: "Sword",
  extends: None,
  members: [
    Field("damage", type: inferred, value: RangeLit(10, 15)),
    Field("crit_chance", type: inferred, value: PercentLit(5)),
    Field("weight", type: inferred, value: UnitLit(3, "kg")),
    Field("rarity", type: inferred, value: Ident("common")),
    Method(
      name: "on_hit",
      params: [Param("target", type: inferred, default: None)],
      body: [
        ExprStmt(MethodCall(
          recv: Ident("target"),
          method: "damage",
          args: [MethodCall(
            recv: FieldAccess(Ident("self"), "damage"),
            method: "roll",
            args: []
          )]
        ))
      ]
    )
  ]
)
```

This AST is the canonical, round-trippable form. The formatter regenerates source from this AST; the bytecode compiler (Phase 3) will consume it as input.

---

## Appendix B — Open design questions

A non-exhaustive list of unresolved questions that need answering before Phase 1 is locked:

1. ~~**Should `say <actor>: "..."` be a keyword form or a special-cased method call?**~~ **Resolved 2026-04-29:** keyword form. `say` is lexed as a keyword and parsed as `Stmt::Say { actor: Option<Expr>, text: Expr }`. Both forms (`say "text"` and `say <actor>: "text"`) work. Phase 5 task 3 ships this in the tree-walker.
2. **Should `then` be a real keyword?** It appears in `telegraph(...) then if player.in_zone: ...` (Example 10). May be a parse hazard. Tentative: yes, but only in expression position after specific runtime functions that return a future.
3. **Should percent be its own type or a `float` with a unit?** Tentative: own type (`percent`), with explicit conversion. Cleaner semantics.
4. **Should array literals use `[...]` or `list(...)`?** `[...]` is universal. Going with `[...]`.
5. **Should the language enforce one indentation width per file, or be flexible?** Flexible within a project, but the formatter normalizes to four spaces. Mixing tabs and spaces is rejected.
6. **Should `entity` instances be reference-typed or value-typed?** Reference-typed (entities are heavyweight). Plain records (`{ x: 1, y: 2 }`) are value-typed.
7. **Should there be a `const` keyword for compile-time constants?** Tentative: no; `let` at module scope with a literal initializer is implicitly const-evaluable. Promote to `const` only if needed.

These will be resolved during Phase 1 implementation, with each answer documented in a design-change note.

---

## Document history

- v0.1.0-pre, 2026-04-27: initial draft. Sections 1–10 complete; section 11 listed; appendices A and B drafted.

---

*This is the spec. Everything else exists to support it.*
