The player starts at the origin and moves with W/A/S/D at 3 units per
second along each axis (D +x, A -x, S +z, W -z). Pressing R rewinds time
for the player: it jumps back to where it was 1 second (60 ticks) ago,
or to the oldest position remembered if that's less than a second back.
Positions are remembered from the start, one per tick; after a rewind,
the positions it skipped over are forgotten (rewinding again goes
further back from there).
