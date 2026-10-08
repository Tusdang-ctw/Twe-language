# The Twe build

The Twe side of this same-game comparison is the existing reference game
[`examples/action3d`](../../action3d/) — not copied here, to keep one
source of truth.

- Play it: `twec play3d examples/action3d/main.twe`
- Build for the web: `twec build --target web examples/action3d`

It is **229 lines of Twe in one file** (`main.twe`) plus a 7-line
`twe.toml`, no dependencies, and the build is a single command with no
external toolchain. The Three.js re-implementation of the *same game*
lives in [`../threejs/`](../threejs/).

See [`../README.md`](../README.md) for the side-by-side comparison.
