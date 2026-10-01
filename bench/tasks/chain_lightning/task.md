The player stands at the origin. Enemies with 5 health stand still at
(2, 0, 0), (4, 0, 1), (6.5, 0, 1), (3, 0, -2.5) and (9, 0, 0). Pressing
Space casts chain lightning:

1. It strikes the enemy nearest the player, if one is within 5 units,
   for 4 damage.
2. It then jumps from the enemy it just struck to the nearest enemy that
   this cast hasn't struck yet, if one is within 3 units of it, for 2
   damage;
3. and once more the same way, for 1 damage.

An enemy at 0 health is removed (a removed enemy's position still counts
as the jump's starting point), and counts as a kill. Draw the enemies.
