# Camera projection, field of view, and a proportional 3D HUD (Lanternfall E1)

**Date:** 2026-11-03
**Line:** Lanternfall engine enablers, E1 (first of E1–E6; plan in the
session notes, memory `project_lanternfall`)
**Status:** shipped.

## Why

Lanternfall is a top-down 3D battle royale: the player aims with the mouse,
bots carry name tags and health pips, hits throw up floating damage
numbers. All three need a mapping between the world and the screen, and
Twe had none:

- **No projection.** A script could set `camera.eye` / `target` / `up` /
  `far`, but nothing mapped a world point to the HUD or the cursor to the
  world. A script couldn't even do it by hand: the projection used the
  window's aspect, which only the renderer knew.
- **A fixed lens.** The 60° field of view was hard-coded.
- **A stretched HUD.** The 3D HUD was the 2D game's 640×480 canvas
  stretched over the window, so at 16:9 every circle was an ellipse and
  every letter a third too wide.

No example forced a fix before; the reference battle royale does (Principle
5: the engine's camera should be a first-class value, not something a
script reverse-engineers).

## What ships

```twe
camera.fov = 45.0                                    # degrees, default 60
let tag = camera.project(bot.pos + vec3(0, 2.2, 0))  # (x, y) on the HUD, or nil behind the camera
let aim = camera.ground_point(mouse.pos, 1.0)        # vec3 where the cursor meets y = 1, or nil
let r = camera.ray(mouse.pos)                        # {origin, dir}
screen.aspect                                        # window width / height
```

- `camera.project` returns `nil` only for points behind the camera. Points
  off the side of the view still project outside the canvas, so a script
  can clamp them to an edge arrow.
- `camera.ground_point` returns `nil` when the ray runs parallel to the
  plane or meets it behind the eye.
- **One camera.** The new `src/camera3d.rs` holds `Camera3d`,
  `perspective`, `look_at` and `mul`, moved out of the renderer. The kernel
  draws with them and the builtins compute with them, so the script's
  projection and the GPU's can't drift. `read_camera` moved there from
  `host3d.rs` and gained `camera.fov` (clamped to 5–170°).
- **The window size is input.** Both 3D shells report it to
  `InputState::set_view` (native on `Resized` and at window creation; web
  every frame from the canvas, 3D games only). The tick's `InputCommand`
  carries it as `view_w` / `view_h`, and `apply_command` sets the
  viewport and `screen.size` / `center` / `aspect`. Replay logs are now
  `TWE-REPLAY v3`, with the view as two extra fields, so a replayed run
  projects the same way. v2 and v1 logs still play (view unknown, kept
  at 640×480).
- **A proportional 3D HUD.** In 3D the HUD canvas is 480 units tall and
  `480 × aspect` wide (`camera3d::hud_size`): 640×480 at 4:3, about 853×480
  at 16:9.
  - `Canvas::set_units` lays the HUD out in those units. The 2D path keeps
    640×480.
  - Shells hand the cursor to `InputState::pointer` as a fraction of the
    view, which scales it to whichever canvas the game uses: 640×480 for 2D
    hosts (which never set a view), the aspect-wide one for 3D.
  - Native 3D touch points use the same mapping.

## Compatibility

- **Nothing changes at 4:3.** The browser page keeps its canvas at 4:3
  (`web/index.html`), and the native 3D window opens at 640×480. Every
  shipped 3D example looks as before unless a native window is resized.
- **Resized native windows.** For that case `examples/survive3d` now
  sizes its full-screen dimmers with `screen.size` and right-aligns its
  wave and kill counters from `screen.size.x`. Its menus stay laid out in
  a 640-wide area at the left.
- **Not yet proportional:** web 3D touch points still use 640×480 units.
  3D-in-the-browser touch has no user yet.

## Tests

- **`tests/camera3d.rs`** (no GPU needed):
  - HUD sizes;
  - the target projects to the centre at 4:3, 16:9 and portrait;
  - `ground_point` → `project` round trips;
  - `ray` passes through what `project` maps;
  - the `nil` cases;
  - `fov`;
  - the script surface end to end through `sim_tick` (`screen.size`,
    `screen.aspect`, `camera.ray`, `nil` behind the camera).
- **`tests/kernel_render.rs::camera_project_lands_on_the_drawn_pixel`:**
  in a 480×270 headless view, the sphere's pixel is where the script's
  `camera.project` says.
- **`replay::tests::v3_carries_the_view_and_v2_still_plays`.**

## Docs

`docs/06` §7.8 (world ↔ screen, the proportional 3D HUD) and §7.6
(mouse units in 3D).
