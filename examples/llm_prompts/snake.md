# Task: Snake (single-file, scene + state machine)

Write a complete Twe program that implements the classic Snake game.

## Constraints

- Indentation-based syntax (no braces, no semicolons).
- One `scene Snake:` declaration containing all state.
- A grid 32 cells wide × 24 cells tall, each cell 20 pixels.
- Arrow keys change direction (`key_held("up")`, etc.) — never reverse
  into yourself.
- Eating food (a single random cell) grows the snake by one segment
  and re-rolls the food's location.
- Hitting a wall or your own body transitions to a `game_over` state.
- The `game_over` state shows "GAME OVER — press R to restart" and
  transitions back to `playing` on `key_held("r")`.

## API hints (from the stdlib manifest)

- `rect(at: (x, y), size: (w, h), color: ...)` — draw a filled rectangle.
- `text("...", at: (x, y), size: 20, color: color.white)` — draw text.
- `random.int(0..N)` — uniform int in `[0, N)`.
- `key_held(name)` / `key_pressed(name)` — keyboard input.
- `0..32` is a half-open range, so `random.int(0..32)` returns 0..31.

## Output

Reply with **only** a single ```twe fenced block. No commentary
outside the fence. After your reply, `twec verify` runs on it; if it
errors, the structured `fix` payload comes back to you in the next
round — apply it mechanically.
