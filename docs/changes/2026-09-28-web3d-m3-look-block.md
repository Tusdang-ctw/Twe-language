# Design change: the `look:` block

**Date:** 2026-09-28
**Milestone:** web3d-M3 (plan: [`2026-09-27-web3d-pivot.md`](2026-09-27-web3d-pivot.md); evidence: [`2026-09-28-web3d-m3-baseline.md`](2026-09-28-web3d-m3-baseline.md))
**Status:** accepted; implemented in stages within M3 (see "Staging").

## Problem

In 3D today, an entity has no way to say what it looks like:

- Per-entity `render()` methods are silently ignored by the 3D path, so a script has to loop over its entities in the top-level `on render():` and call `cube()` / `sphere()` / `mesh()` for each one.
- That is a silent footgun (Principle 3), and it is also the most expensive thing a large 3D scene does. On the 5,000-enemy benchmark it costs about 11 ms per frame in Chrome, more than the whole 60 fps budget's share for drawing, because every drawn entity runs script.

The Web3D brief's second law is that rendering is declarative: the simulation says *what* things are, and the renderer decides how to draw them without calling back into game code. Principle 1 says game concepts are language constructs. What an entity looks like is such a concept.

## Decision

An `entity` may contain one `look:` block: a closed set of keys, each bound to an expression.

```twe
entity Enemy:
    var pos = vec3(0, 0, 0)
    var hurt = 0.0

    look:
        mesh: "cube"
        scale: 0.35
        tint: if hurt > 0: color.white else: color.red
```

Every live entity whose class has a `look:` is drawn each frame at its `pos`, with no script call per entity unless a key reads the entity's own state.

### Grammar (LL(1); `docs/06` §3.3)

```
look_block := "look" ":" INDENT (look_key ":" expr NEWLINE)+ DEDENT
look_key   := identifier                     # one of the closed set below
```

`look` becomes a reserved keyword, bringing the count to about 52. That runs against the open "keyword pruning" item, but the alternative is a contextual keyword, which Principle 4 rules out. The one existing use of `look` as an identifier (`examples/fps_demo.twe`) is renamed.

`look:` is only valid directly inside `entity`. Anywhere else is a parse error that says so.

### Keys

| Key | Value | Default | Meaning |
|---|---|---|---|
| `mesh` | string: `"cube"`, `"sphere"`, or a `.glb` path | `"cube"` | what to draw; paths resolve like `mesh()` |
| `tint` | color tuple `(r, g, b)` or `(r, g, b, a)` | white | multiplies the mesh's color / texture |
| `scale` | number | `1.0` | uniform size, as `size:` in `cube()` |
| `facing` | number (radians about +Y) | `0` | yaw; M3 stage 2 |
| `material` | a `visual` block name | none | procedural surface; M3 stage 3 |

- The set is closed. `twec verify` rejects an unknown key, suggests the closest known one, and rejects duplicates.
- Keys not yet implemented (`facing`, `material`) are rejected with a message naming the M3 stage that adds them. They are never accepted and then ignored.

### Semantics (`docs/06` §4.9a)

- **Position** is the entity's `pos` field, a `vec3`. An entity drawn without a `vec3` `pos` is a runtime error that names the class. `spawn X at p` already sets `pos`.
- **Scope.** Key expressions resolve like a method body: the entity's fields and `self` are visible, as are globals.
- **When keys are evaluated.** Classification is static, from the resolver:
  - A key is **per-entity** if its expression reads `self`, a field, or calls anything other than a pure builtin. It is evaluated for each drawn entity, each frame.
  - Otherwise it is **shared**: evaluated once per class per frame. A shared key can still read globals, so it can change over time. It just doesn't vary between entities.
  - Either way the results equal evaluating every key for every entity, because a shared expression can't observe which entity it's evaluated for. The classification only removes redundant work.
- **Inheritance.** Keys merge along the `extends` chain, and a subclass overrides individual keys. `BigSlime extends Slime` can set only `scale`.
- **Drawn:** live entities, including while paused. Despawned entities are not drawn. Order among looks is unspecified; the depth buffer resolves it.
- **Top-level `on render():`** still runs and still draws. `look:` adds to it; it doesn't replace it.
- **In 2D**, the macroquad player doesn't implement `look:` until web3d-M6. Running a program that declares one there is an error at startup ("`look:` is 3D-only until…"), not a silent no-draw.

## Alternatives rejected

- **Honour per-entity `render()` in 3D.** It is imperative: it runs script per drawn entity every frame, which is the measured cost this removes, and it can't feed the M3 instance pipeline without re-running script. It also gives two obvious ways to draw an entity (Principle 2). In 3D it stays ignored. A `verify` warning pointing at `look:` is a follow-up.
- **Keys as bare identifiers** (`mesh: cube`). That would make `cube` a name lookup (today it's the `cube()` builtin) or a context-sensitive token. Strings are unambiguous and legible to LLMs.
- **Declarative only (constants).** Games need state-driven looks: a hit flash, a heading. Allowing expressions, and evaluating only the entity-dependent ones per entity, keeps the common constant case at zero script cost.

## Stage 1 results (2026-09-28)

On `examples/swarm_3d.twe` (5,000 enemies), with the same scene before and after the switch to `look:`:

| | Before (`on render():` loop) | After (`look:`) |
|---|---|---|
| script-side drawing, native | 3.56 ms (713 ns/entity) | **0.47 ms (93 ns/entity)** |
| script-side drawing, Chrome | 10.7–12.1 ms per frame | **1.0 ms per frame** |
| Chrome frame rate | 39–49 fps | **114–127 fps** |

The update tick is unchanged by this stage: 9.4–10.5 ms per tick in Chrome, against a target of 4 ms. That is M3 step 2.

## Staging

1. **Stage 1:** `mesh` / `tint` / `scale`, drawn through the existing instanced path. Full integration across the lexer, parser, printer, AST JSON, resolver, inference, verify, grammar export, tree-sitter and TextMate. `swarm_3d` moves to `look:`, and the baseline is re-measured.
2. **Stage 2: done (2026-09-28).** `facing` is a yaw in radians about +Y, where `0` faces +Z. The kernel's instance data carries (sin, cos), and both the main and shadow vertex shaders rotate position and normal, so shadows turn with the mesh. `math.atan2(y, x)` was added so a script can turn a direction into a facing. `tests/kernel_render.rs::look_facing_rotates_the_mesh` checks it on the GPU: a cube turned 45° changes its footprint, and one turned 90° matches the unturned cube.
3. **Stage 3:** `material` (`visual` blocks as mesh surfaces).
