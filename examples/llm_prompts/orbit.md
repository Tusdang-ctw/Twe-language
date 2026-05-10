# Task: Orbiting projectile (single entity + update loop)

Write a Twe entity that orbits a fixed point on the screen.

## Constraints

- Indentation-based syntax. No braces, no semicolons.
- One `entity Orbiter:` declaration with `var pos = (320.0, 240.0)`
  and `var angle = 0.0`.
- `function update(dt):` advances `angle` by `2.0 * dt` and computes
  `pos` as the center `(320, 240)` plus `(cos(angle) * 100,
  sin(angle) * 100)`.
- `function render():` draws a `rect` 10×10 centered at `pos` in
  `color.cyan`.
- Top-level `spawn Orbiter at (320, 240)` to put one in the world.

## API hints

- `math.cos(x)`, `math.sin(x)` — trigonometry, x in radians.
- `rect(at: ..., size: (10, 10), color: color.cyan)`.

## Output

Reply with **only** a single ```twe fenced block. Twe is whitespace-
sensitive; indent each block with 4 spaces.
