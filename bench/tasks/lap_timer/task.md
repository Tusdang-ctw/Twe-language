A race track has four checkpoints, in order: A at (8, 0, 0), B at
(8, 0, 8), C at (0, 0, 8), and the finish at (0, 0, 0), where the car
starts. A checkpoint is reached when the car comes within 1 unit of it,
and only the next checkpoint in order can be reached (passing near a
later one does nothing). Reaching the finish after A, B and C completes a
lap: its time is the time since the lap began (the first lap begins at
the start; each next lap begins when the previous one completes). Keep
the number of laps, the last lap's time and the best lap time. The car
moves with W/A/S/D at 4 units per second along each axis (D +x, A -x,
S +z, W -z).
