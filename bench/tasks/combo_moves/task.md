A fighting-game input reader turns key presses into moves:

- **Fireball:** Down, then Right, then Space, each press within 0.5
  seconds of the one before.
- **Dash:** Right, then Right again within 0.3 seconds. The second Right
  completes the dash and can't be the first Right of another dash.
- **Punch:** a Space press that doesn't complete a fireball.

Count each move. Only the presses matter (the order of keys pressed, and
the time between them); keys held don't.
