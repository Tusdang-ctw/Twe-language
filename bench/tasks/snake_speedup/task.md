You are given a working program for this game:

Snake on a 10 x 10 grid of cells, x and y from 0 to 9. The snake starts
3 cells long: head at (4, 5), then (3, 5) and (2, 5), moving right (+x).
It moves one cell every 0.25 seconds, the first move 0.25 seconds after
the start. The arrow keys turn it (up is -y, down is +y), but it can't
turn straight back onto itself. Food appears at (7, 5), then (7, 2),
then (2, 2), then (2, 8), one at a time: when the head reaches the food,
the snake grows by 1 cell and the next food appears (after the last
there is none). Moving into a wall or into itself ends the game: the
snake stops where it was. Draw the snake and the food.

Change it: Each food eaten makes the snake faster: the time between moves drops by
0.05 seconds per food (0.25, then 0.2, then 0.15, ...), timed from the
move that ate it.
