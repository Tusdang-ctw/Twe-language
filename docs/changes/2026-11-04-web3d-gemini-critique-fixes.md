# Design Change Note — vectors vs colours, `music.is_playing`, `attach:` (2026-11-04)

**Status:** shipped (tree-walker; native + web share it).
**Trigger:** an outside review of Twe against Three.js (Gemini, from a screenshot of `examples/audio_demo.twe`). Most of its points described a 2D Twe that no longer exists. Three survived a check against the code, and this note closes them.

## 1. A vector is not a colour (Principle 3)

**Problem.** `vec3(...)` and `color.*` both produced bare tuples, so `pos + color.red`, `tint: pos` and `color.lerp(dir, ...)` ran silently and drew nonsense. The review's version of this ("a 4-tuple could be a quaternion") was wrong: `quat` is its own value. The colour/vector half was right.

**Decision.** Tuples carry a one-byte `TupleKind`: `Plain`, `Vector` or `Color` (`src/tagged_value.rs`).
- **Where kinds come from.** `vec3` and position-returning builtins make vectors; the `color.*` constants and constructors make colours; a literal is plain.
- **What gets refused.** Arithmetic (`+`, `-`, `mix`) joins kinds, with plain taking the other side's, and refuses vector + colour. Scaling keeps the kind. The colour decoders (`rgba`, `color_of`, look `tint:` / `glow:`) refuse vectors; `xy_of` (2D `at:` / `size:`) refuses colours. `xyz_of` and `rgba_of` already refused the other kind by length (a colour is 4 numbers, a vec3 is 3).
- **Invisible otherwise.** Equality, display, indexing and saves ignore the kind.

**Alternatives rejected.**
- *Nominal `Vec3` / `Color` types with methods.* That would add a second way to call vector maths (methods next to `math.*`, against Principle 2) and change how every example prints.
- *A static check in `infer`.* `infer` has no per-builtin signatures, so it can't know `vec3` returns a vector. That table is the prerequisite; until it exists, the check runs at run time, like the tuple-length errors it sits next to.

**No false positives.** Only values that came from `vec3` / `color.*` are checked, so a program that ran before still runs unless it really mixed the two. Every example and test program still runs (`tests/examples_run.rs`).

**Cost.** The kind lives in existing padding: a `SmallTuple` (≤3 elements) is still one 32-byte heap body, and `HeapBody`'s size is unchanged.

## 2. `music.is_playing(handle)`

**Problem.** `audio_demo.twe` kept a `music_playing` flag in sync with `music.play` / `music.stop` by hand. Two independent records of one fact are a footgun.

**Decision.** `music.is_playing(handle)`: true from a looped play (`music.play`, `music.play_at`, a `music.layer` starting) until a stop (`music.stop`, `sound.stop`, the end of a `music.crossfade` away from it). It reads the simulation's own record (`audio_polish::start_looping` / `stop_looping`), never the audio device.

**Why not `sound.is_playing` for one-shots.** Only the audio device knows when a one-shot ends. A script that branched on it would differ between runs, machines and the browser, which breaks replays and the rollback work. Looped tracks don't have this problem: they play until told to stop.

**Not "handles as objects".** The review suggested `track.stop()` / `track.volume = v`. That would be a second calling convention next to every other `namespace.fn(handle)` in the stdlib (Principle 2). The footgun was the flag, not the call shape.

**Side fix.** A hot reload already "started the mixer fresh" (`audio_polish::clear`), but it never stopped looped tracks, so a reloaded script that called `music.play` again played the track twice. It now stops them, which also keeps `music.is_playing` true to what is heard.

## 3. `attach:` / `offset:` in looks (parent-child transforms)

**Problem.** `look:` had no hierarchy. Lanternfall's `HeldGun` copied `owner.pos` and `owner.heading` in `update` and rebuilt the owner's basis by hand (three lines of trig). Because entity `update` order is spawn order, the copy could lag a tick behind its owner.

**Decision.** Two look keys: `attach: <entity or nil>` and `offset: <vec3 in the parent's frame>`.
- **Where it is drawn.** `at = parent.at + q_parent * offset`, and the orientation is `q_parent * facing * rotation`, matching the renderer's `facing then rotation` order.
- **Resolution.** Done at draw time, after every per-entity look is evaluated, so chains work in any spawn order, and the parent's place is the one actually drawn this frame, so there is no lag.
- **Scale is not inherited.** A non-uniform parent scale composed with a child rotation is a shear, which a draw's (rotation, scale) can't express. An attachment is a rigid mount.
- **What a parent can be.** One with no look contributes its `pos`; one with a shared look contributes its class values.
- **Despawns and errors.** A despawned parent hides the child. A loop, or a chain deeper than 16, is an error.
- **Only the drawing moves.** The child's `pos` isn't touched, so gameplay reads stay explicit.

**Forced by an example.** `examples/lanternfall/loot.twe` `HeldGun`, now `attach: owner` + `offset: vec3(-0.12, 0.86, 0.42)` (−2 fields, −4 lines, no lag); `tests/lanternfall.rs` checks the drawn place against the old basis.

**Deferred.**
- Attaching to a skeleton joint (a gun in an animated hand bone) needs the joint palette on the CPU side.
- A way to read an attached look's world position from script.
- Inheriting scale.

## Tests
- `tests/programs/tuple_kinds.twe` + `vectors_and_colours_mix_with_plain_tuples`, `a_vector_is_not_a_colour_and_a_colour_is_not_a_position`, `a_look_refuses_a_vector_tint_or_glow`.
- `music_is_playing_follows_play_and_stop`.
- `tests/programs/look_attach.twe` + `look_attach_rides_the_parent_frame`, `look_attach_errors_name_the_problem`; `held_guns_follow_their_fighters` (Lanternfall).

## Docs
`docs/06` §2.5.7 (vectors and colours), §4.9a (`attach` / `offset`), §7.4, §7.10 (`music.is_playing`; the section's examples also showed `sound.play_at(sfx, (x, y))` and `sound.volume`, which never existed, and now show the real calls).
