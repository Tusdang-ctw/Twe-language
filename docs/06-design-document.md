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

### 4.7 Events

The `on <event>:` form registers an event handler. There are three event kinds:

1. **Frame events:** `on update(dt):`, `on render():`. Fire every frame.
2. **Predicate events:** `on hp < 20%:`. Fire when the predicate transitions from false to true.
3. **Named events:** `on enemy.death(e):`. Fire when something publishes that event.

Handlers are scoped:

- At the top level, they run for the entire program.
- Inside a `state` block, they run only while in that state and are deregistered on state exit.
- Inside an `entity` declaration, they run for the lifetime of that entity instance.

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

## 7. Standard library overview

Detailed in a separate document (`stdlib.md`, not yet written). Sections planned:

1. **`core`** — `print`, `assert`, `panic`, type predicates.
2. **`math`** — `sin`, `cos`, `noise`, `smoothstep`, `mix`, `clamp`, `floor`, `ceil`, etc. (most also work in `visual` blocks).
3. **`random`** — `random.float()`, `random.int(0..10)`, `random.in_circle(radius:)`, `random.choice([...])`.
4. **`vector`** — vector operations: `dot`, `cross`, `length`, `normalize`, `lerp`.
5. **`color`** — named constants, `color.lerp`, `color.from_hex`, `color.hsv`.
6. **`time`** — `time.now`, `time.delta`, `time.frame`. *(v0.1 ships only `time.dt`, the live frame delta — set by `eval::tick_frame` and readable from every-clock bodies; closes Phase 2 frustration F8.)*
7. **`input`** — `key`, `mouse`, `gamepad`.
8. **`scene`** — `scene.find`, `scene.spawn`, `scene.npc`, `scene.enter`.
8a. **`entities`** *(v0.1)* — `entities.of(Class)` returns a list of live instances of a class; `entities.count(Class)` returns just the count. Closes the iteration-on-dynamic-instances gap from the Phase 2 frustration list (F3 / F6) and is what bullet/enemy collision in `examples/survive.twe` is built on.
9. **`asset`** — `load`, `load_mesh`, `load_sound`. *(v0.1: bare `load(path)` is canonical for sprites — matches Example 1 in `01-examples.md`. Returns a sprite handle `{ path, x, y }`; the texture itself is decoded lazily on the first `sprite(handle, at, [size])` call inside `on render():`. Path existence is checked at load time so typos fail fast.)*
9a. **`sound`** *(v0.1)* — `sound.load(path)` returns a sound handle `{ path }`; `sound.play(handle)` decodes (lazily, cached) and plays it once. WAV and Ogg Vorbis supported (via `quad-snd`). Audio is enabled via macroquad's `audio` feature.
10. **`io`** — `read_file`, `write_file`, `save_to`, `load_from`.
11. **`net`** — minimal HTTP for fetching at dev time; full networking deferred.

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

1. **Should `say <actor>: "..."` be a keyword form or a special-cased method call?** Keyword form is more readable; special-cased call is more uniform. Tentative: keyword form, only legal inside `dialogue` blocks.
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
