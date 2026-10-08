# Same game, two stacks: Twe vs Three.js

This is the **human-authoring** half of web3d-M12 — the measured proof for
the claim that Twe is a better way to *build* a browser 3D game. The *same*
game is implemented twice:

- [`twe/`](twe/) → the existing reference game [`examples/action3d`](../action3d/) — a 3rd-person action slice: a hero with blend-space locomotion + a one-shot sword attack, 8 chasing enemies, a spherical-joint rope bridge, IBL + sun shadows + AO + bloom, and a chase camera.
- [`threejs/`](threejs/) → a faithful, idiomatic Three.js re-implementation of that exact game.

The two use the **same physics engine** (rapier — `rapier3d` natively in
Twe, `@dimforge/rapier3d-compat` in Three.js), so the physics half is
like-for-like, not a rigged contest.

> **Scope.** This is the *human-authoring* axis only. M12's other axis —
> can an **LLM** build the game better in Twe than in Python/JS — is the
> parked M5 benchmark and needs the maintainer's API key + budget before it
> runs. Until then the LLM-moat claim stays unproven (and has a
> pre-registered kill line). This document does not speak to it.

## What is verified

- **Twe:** `examples/action3d` runs natively (`twec play3d`) and builds for
  the browser (`twec build --target web`); it is covered by the suite
  (`tests/survive3d.rs` skinned-mesh guard, `tests/physics_joints.rs`,
  `tests/physics_web.rs`) and verifies 0/0.
- **Three.js:** `npm install` + `vite build` is clean (27 modules, no
  errors), and a headless real-Chrome smoke
  (`vite preview` + playwright) loads the page, initialises rapier, builds
  the full scene (IBL from the HDR, sun shadows, GTAO, bloom, the skinned
  hero + its clips, the 8 enemies, the jointed bridge) and runs its render
  loop **with zero JavaScript errors**, reaching `status: ready` and
  advancing frames. See [`threejs/README.md`](threejs/README.md) to run it.
- A pixel-level visual comparison of the two is a GPU + human-eyes step
  (the same visual-playtest deferral `action3d` already carries); it is not
  claimed here.

## The measurement

Objective, counted — not estimated:

| Metric | Twe (`action3d`) | Three.js | Ratio |
|---|---:|---:|---:|
| Game code (lines) | **229** | 452 | 2.0× |
| Source files (hand-written) | **1** (`main.twe`) | 3 (`main.js`, `materials.js`, `index.html`) | — |
| Config / scaffold files | **1** (`twe.toml`, 7 lines) | 4 (`package.json`, `vite.config.js`, `index.html` shell, `.gitignore`) | — |
| Runtime dependencies | **0** | 3 direct / **13** npm packages | — |
| Build toolchain | `twec` (one static binary) | Node + npm + Vite | — |
| Build command | `twec build --target web <dir>` | `npm install && vite build` | — |
| Shipped JS | — (one wasm engine) | 2.76 MB (941 KB gzip) | — |

(Line counts: `wc -l`. The Three.js "game code" row is `main.js` 358 +
`materials.js` 94; the 30-line `index.html` — which also carries the HUD
that Twe draws with two `text()` calls — is listed under files, not doubled
into the code row.)

## Where the lines went — feature by feature

Every row is the *same* visible behaviour in both games.

| Feature | Twe | Three.js |
|---|---|---|
| **Locomotion blend** | one call: `mesh_anim.blend_space(hero, ["idle","walk","run"], [0, 3.2, 6.5], speed)` | hand-weight three `AnimationAction`s each frame, bracket the speed, and manually copy `walk.time → run.time` to keep gaits phase-synced (`driveBlendSpace`, ~25 lines) |
| **Procedural materials** | a `visual` block: `surface(uv, time, pos) -> material(...)`, type-checked, compiled to WGSL | `onBeforeCompile` string-injection into `MeshStandardMaterial`, tied to three's internal `#include` names, no type-checking, plus a hand-written value-noise GLSL (`materials.js`, 94 lines) |
| **IBL** | `light.environment("sky.hdr", 0.55)` | `RGBELoader` → `PMREMGenerator.fromEquirectangular` → `scene.environment`/`background`/`environmentIntensity` (dispose both) |
| **Sun + shadows** | `sun.direction(...)`, `sun.shadow(true)`, `sun.shadow_extent(20)` | a `DirectionalLight`, normalise+scale its position, `castShadow`, configure the ortho shadow-camera frustum + bias + map size |
| **Post (AO + bloom)** | `postfx.ao(0.9)`, `postfx.bloom(0.3)` | build an `EffectComposer`: `RenderPass` + `GTAOPass` + `UnrealBloomPass` + `OutputPass`, and keep its size in sync on resize |
| **HUD** | two `text(at:, content:, ...)` calls in `on render()` | a DOM overlay in `index.html` + CSS + `textContent` updates |
| **Physics bridge** | `physics.static_box`, `physics.body`, `physics.spherical_joint`, draw at `physics.position(h)` | the *same* rapier calls (`createRigidBody`, `ColliderDesc.cuboid`, `JointData.spherical`, `createImpulseJoint`), plus manual per-frame mesh↔body transform sync — comparable, since it is the same engine |
| **Entities / game loop** | `entity` + `scene`/`state` + `on update(dt)` as language constructs | plain objects, arrays, and a `setAnimationLoop` callback |

## The honest read

**On this axis, Twe wins clearly where its five principles bite.** Half the
code, one file, zero dependencies, and a one-command build with no
toolchain. The biggest gaps are exactly the wedges: *game concepts are
first-class* (a blend space, a procedural surface, a HUD draw, a `scene`
state are one construct each, not an assembled pipeline), and *one obvious
way* (there is one blend-space call, not a choice of how to weight actions).
The determinism and the single self-contained artifact are structural, not
stylistic.

**Three.js wins where it was always going to.** It is not close on
ecosystem, maturity, flexibility, tooling, or training corpus: `GLTFLoader`,
`OrbitControls`, gizmos, inspectors, a decade of Stack Overflow, and an
escape hatch for anything the abstraction does not cover. A team that needs
the thing Twe has no construct for reaches for it in Three.js in minutes;
in Twe they may have no path at all. The `onBeforeCompile` row cuts both
ways — it is painful, but it *exists*, and Twe's `visual` block cannot
express everything a raw shader can.

**What this does and does not settle.** It substantiates the *authorability*
wedge with counted numbers on a real, running, non-trivial game. It does
**not** settle "the best way to build a browser 3D game" on its own: that
also rests on the unmeasured LLM-authoring axis (the parked M5 runs) and on
the ecosystem axis, where Three.js leads. The defensible claim today is
narrow and real: **for a human authoring a self-contained browser 3D game
of this kind, Twe takes about half the code, no dependencies, and no build
toolchain — and the Three.js equivalent, built faithfully, confirms it.**

## Reproduce

```sh
# Twe
twec play3d examples/action3d/main.twe
twec build --target web examples/action3d

# Three.js
cd examples/threejs-parity/threejs
npm install
npm run dev        # play it
npm run build      # measure dist/
```
