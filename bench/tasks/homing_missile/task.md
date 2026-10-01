A target starts at (10, 0, 0) and moves along +z at 2 units per second.
Pressing Space launches a missile from the origin, heading along +x at 5
units per second, but only if no missile is in flight. Each tick the
missile turns toward the target by at most 180 degrees per second (it
keeps its speed) and then moves. When it comes within 0.5 units of the
target, it hits: the missile is gone and the hit is counted (the target
keeps moving). A missile that hasn't hit within 6 seconds is gone too.
