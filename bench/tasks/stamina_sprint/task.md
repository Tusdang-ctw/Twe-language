The player starts at the origin and moves along +x while D is held, at 4
units per second. Holding Shift as well makes it sprint at 8 units per
second, which uses stamina. Stamina starts at 100 (the maximum).

- While sprinting, stamina drains at 30 per second. When it reaches 0 the
  player is exhausted: it can't sprint (D still walks at 4) until stamina
  has climbed back to 50.
- Whenever the player isn't sprinting, stamina refills at 20 per second,
  up to 100.

Sprinting means: D and Shift are held, the player isn't exhausted, and
stamina is above 0.
