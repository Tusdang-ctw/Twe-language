The player stands still at the origin. An enemy with 3 health starts at
(6, 0, 0) and walks straight toward the player at 1.5 units per second.
Pressing Space attacks, if the attack's 0.4-second cooldown has passed
(presses during the cooldown do nothing). An attack hits every enemy
within 2 units of the player: it loses 1 health and is knocked back. A
knocked-back enemy moves straight away from the player starting at 8
units per second, slowing down by 16 units per second every second until
it stops; while knocked back it doesn't walk. An enemy at 0 health is
removed. Draw the enemy.
