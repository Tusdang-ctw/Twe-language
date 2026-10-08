# action3d — Three.js build

A faithful, idiomatic Three.js re-implementation of Twe's
[`examples/action3d`](../../action3d/), for the web3d-M12 same-game
comparison. See [`../README.md`](../README.md) for the side-by-side.

## Run

```sh
npm install
npm run dev       # dev server, open the printed URL
npm run build     # production build into dist/
npm run preview   # serve the production build
```

Node 18+. Needs network access for `npm install` (three, rapier, vite).

## What it is

- `src/main.js` — the game: renderer, IBL, sun + shadows, the AO/bloom
  `EffectComposer`, the GLTF hero + `AnimationMixer`, the by-hand
  locomotion blend space, the chasing enemies, the chase camera, and the
  rapier rope bridge.
- `src/materials.js` — the two procedural materials (grass ground, enemy
  blob) as `onBeforeCompile` patches on `MeshStandardMaterial` + hand-written
  value-noise GLSL. In Twe these are two short `visual` blocks.
- `index.html` — the page shell and the DOM HUD overlay.
- `public/assets/` — `hero.glb` (the animated character) and `sky.hdr` (IBL).
  The ground and enemies are a plane + spheres, so no mesh assets are needed
  for them.

The physics uses `@dimforge/rapier3d-compat` — the same rapier engine Twe
uses, so the bridge is a like-for-like comparison.

## Controls

WASD move · Shift run · Space / left-click attack · E shoves the bridge.
