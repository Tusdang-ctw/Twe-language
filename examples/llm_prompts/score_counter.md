# Task: Score counter (UI widgets + state)

Write a Twe scene that shows a numeric score and two buttons (`+1` and
`Reset`) that mutate it.

## Constraints

- Indentation-based syntax. No braces, no semicolons.
- One `scene ScoreScene:` with `var score = 0`.
- One `state ui:` containing an `on render():` block.
- The render block draws:
  - a label showing `"Score: {score}"` at `(20, 20)` size `(200, 30)`,
  - a `button(at: (20, 60), size: (100, 36), label: "+1")` that
    increments `score` when clicked,
  - a `button(at: (130, 60), size: (100, 36), label: "Reset")` that
    sets `score = 0` when clicked.
- Use string interpolation: `text("Score: {score}", ...)` is the form
  Twe expects (no `+` concatenation needed for primitives).

## API hints

- `button(at:, size:, label:) -> bool` returns `true` on the click frame.
- `text(content, at:, size:, color:)` — note `content` is positional.
- `label(at:, size:, text:)` — convenient for static text.

## Output

Reply with **only** a single ```twe fenced block.
