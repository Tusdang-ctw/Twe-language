A guard patrols between (0, 0, 0) and (10, 0, 0) at 2 units per second,
starting at (0, 0, 0) and heading for (10, 0, 0); at each end it turns
around. While patrolling it faces the way it walks. The guard sees the
player when the player is within 4 units and at most 60 degrees to
either side of the direction the guard faces. Seeing the player makes the guard "alert": it
stops and faces the player (turning at once), every tick it sees them.
Once it hasn't seen the player for 2 seconds it goes back to "patrol",
heading for the same end as before. The player starts at (5, 0, 2) and
moves with W/A/S/D at 3 units per second along each axis (D +x, A -x, S
+z, W -z).
