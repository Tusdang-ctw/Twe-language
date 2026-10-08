# web3d: splitting the `stdlib.rs` god-module (first cut — physics)

**Date:** 2026-10-08
**Line:** web3d engineering-health backlog (post-M11)
**Status:** in progress — the module pattern is established and the first
family is extracted. The remaining families are a continuing effort.

## Why

`src/stdlib.rs` had grown to ~13,000 lines — every builtin family (math,
draw, audio, physics, animation, lighting, save, …) in one file. It
compiles and is navigable by section comment, but it is the single
biggest obstacle to a new contributor (and to an LLM editing it: the file
alone is larger than many models' useful edit window). The audit flagged
it (with `kernel/render.rs` at 9.6k) as the top maintainability risk.

This is a mechanical, behaviour-preserving refactor: move cohesive
builtin families into `src/stdlib/<family>.rs` submodules, keeping the
suite green after each.

## The pattern (now proven)

`stdlib` is already a file-plus-directory module (`stdlib.rs` +
`stdlib/experimental.rs`), so extraction needs no restructuring:

1. Move a cohesive family's `fn`s into `src/stdlib/<family>.rs`.
2. The submodule does `use super::*;` — a child sees all of the parent's
   items, including private helpers (`arity`, `number`, `xyz_of`,
   `handle_int`, …) and the parent's type imports (`Value`, `Env`,
   `RuntimeError`, `Object`), so no per-symbol import list.
3. The moved `fn`s become `pub(super)` so the parent's `install()` can
   register them.
4. `stdlib.rs` adds `mod <family>;` + `use <family>::*;`, and `install()`
   references the builtins exactly as before.

A family with items called **outside** `stdlib` (a `pub(crate)` helper the
renderer uses, e.g. `mesh_anim_state`) additionally needs a
`pub(crate) use <family>::<item>;` re-export so `crate::stdlib::<item>`
still resolves. Families whose `fn`s are only referenced by `install()`
(like physics) need none.

## This cut: physics

- **`src/stdlib/physics.rs`** — the 21 `physics_*_impl` builtins plus the
  physics-only `joint_result` helper (the full `physics.*` surface:
  bodies, statics, joints, CCD, character controller, raycast,
  collisions). They were only ever referenced by `install()`, so the move
  is a straight `pub(super)` + `use super::*`, no re-export.
- Shared helpers that happened to sit in the physics section but are used
  file-wide — `handle_int` (generic handle parser, 6 non-physics uses),
  `xyz_of`, `rgba_of`, `texture_handle_id` — stayed in the parent. (That
  interleaving of generic and family-specific helpers is exactly the
  god-file smell this refactor unwinds.)
- `src/stdlib.rs`: ~13,004 → ~12,739 lines.

## Verification

- `cargo check`, `cargo build` — clean (proves `install()` still resolves
  every physics builtin through the new module path).
- `cargo clippy -- -D warnings` native + `-p twe-web --target wasm32` —
  both clean.
- `examples/action3d` verifies 0/0 — the `physics.*` builtin names still
  resolve against the generated manifest, i.e. registration is intact.
- Tests green: `physics_web` (2), `physics_joints` (2), `eval` (226, the
  core language/stdlib suite).

## Second cut: save, mesh_anim, quat

Three more cohesive families moved out in one pass:

- **`src/stdlib/save.rs`** — the `save.*` typed-field helpers (18 fns), no
  items used outside `stdlib`, so a plain `pub(super)` + `use super::*`.
- **`src/stdlib/mesh_anim.rs`** — the `mesh_anim.*` builtins (play / stop /
  blend / `blend_space` / crossfade / advance / current + their list
  helpers + the `MeshAnimEntry` struct). `mesh_anim_state` is `pub(crate)`
  (the renderer reads it), so the parent adds
  `pub(crate) use mesh_anim::mesh_anim_state;`. `MESH_ANIM_STATE` is a
  thread-local in the parent's central `thread_local!` block, so it stays
  put and the module reaches it through `use super::*`; `MeshAnimEntry` is
  `pub(super)` so that thread-local (in the parent) still names it.
- **`src/stdlib/quat.rs`** — the `quat.*` quaternion helpers (7 fns), clean.

`src/stdlib.rs`: ~12,739 → ~12,031 lines (−708 this cut; −973 total since
the split began, across physics + these three). Verified: `cargo check`,
clippy `-D` native + wasm, the inline `mesh_anim` tests (the `super::` paths
resolve through the parent's glob re-export), and the `eval` suite (226).

## Next

Continue family-by-family (remaining clean boundaries: `light`/`light2d`,
`fx`/`tween`, `postfx`, audio scheduling, the `mat4` math + the long
unmarked draw/text/color/input stretch), each a green commit. Then the
harder `kernel/render.rs` split (GPU state + WGSL + pipelines — more
interdependent, a dedicated pass).
