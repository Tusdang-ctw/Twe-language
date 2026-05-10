# Task: Orbiting position via state machine

Write a Twe scene that prints an entity's position once per frame as
it orbits a center point.

## Constraints

- A `scene Orbit:` with `var t = 0.0`.
- One state `running` with `on update(dt):` that:
  - increments `t` by `dt`,
  - prints `"x={...} y={...}"` interpolating
    `(math.cos(t) * 10.0)` and `(math.sin(t) * 10.0)` rounded to one decimal,
  - or simply prints `"frame={t}"` if rounded interpolation is fiddly.
- `initial: running`.

## API hints

- `math.cos(x)` / `math.sin(x)` — radians.
- String interpolation: `print("frame={t}")`.

## Output

Reply with **only** a single ```twe fenced block.
