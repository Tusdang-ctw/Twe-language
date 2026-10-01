A rabbit starts at the origin. The player starts at (-6, 0, 0) and moves
with W/A/S/D at 3 units per second along each axis (D +x, A -x, S +z,
W -z). When the player is within 4 units of the rabbit, the rabbit runs
straight away from the player at 4 units per second; otherwise it stays
still. The rabbit can't leave the square from -8 to 8 on x and on z: a
step that would take it past an edge stops at that edge (on that axis
only).
