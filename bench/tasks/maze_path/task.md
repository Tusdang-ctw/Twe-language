A unit walks on a grid of 8 x 8 cells (x and y from 0 to 7), starting at
cell (0, 0). These cells are walls: x = 1 with y from 0 to 6; x = 3 with
y from 1 to 7; x = 5 with y from 0 to 6. A left click at screen position
(mx, my) picks the cell (mx / 40, my / 40), rounded down. If that cell
isn't a wall, the unit walks there along a shortest path through
non-wall cells, stepping to a side-neighbour every 0.2 seconds, the first
step 0.2 seconds after the click. A new click replaces the destination
(the walk restarts from the unit's current cell). Clicking a wall does
nothing.
