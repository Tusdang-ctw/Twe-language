You are given a working program for this game:

The player has 100 health, which is also the maximum. Pressing H deals
30 damage. Health never goes below 0; at 0 the player is dead, and from
then on nothing changes (no regeneration, H does nothing). Once 2
seconds have passed without taking damage, health regenerates at 10 per
second, up to the maximum.

Change it: S raises a shield for 3 seconds that blocks all damage: a hit on the
shield takes no health and doesn't restart the 2-second wait before
regeneration. The shield can be raised again 10 seconds after it was
last raised; earlier presses of S do nothing. A dead player can't raise
it.
