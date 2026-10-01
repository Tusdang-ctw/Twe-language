The player stands still at the origin with 3 health. Every 1 second
(the first after 1 second) an enemy appears at (10, 0, 0) and walks
toward the player at 3 units per second. Every 1.5 seconds (the first
after 1.5 seconds) the player fires a bolt from the origin toward the
nearest enemy, if there is one; a bolt flies in a straight line at 12
units per second and is removed after 1.5 seconds. A bolt within 0.5
units of an enemy removes both, and counts as a kill. An enemy within
0.6 units of the player is removed and the player loses 1 health. At 0
health the game is over: from then on nothing moves, spawns or fires.
Draw everything.
