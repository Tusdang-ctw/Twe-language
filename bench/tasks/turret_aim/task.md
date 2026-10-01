A turret at the origin starts facing +x (angle 0). Angles are in degrees,
measured from +x toward +z, between -180 and 180. Enemies stand still at
(3, 0, -3), (-5, 0, 0) and (0, 0, 6). The turret always targets the
nearest remaining enemy (by distance from the origin) and turns toward it
the shorter way round at 90 degrees per second, stopping exactly on its
direction. When the turret points within 5 degrees of its target and at
least 0.5 seconds have passed since its last shot (or it hasn't shot
yet), it shoots, and the target is destroyed at once. Count the shots.
