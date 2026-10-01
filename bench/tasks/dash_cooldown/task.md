The player starts at the origin and moves with W/A/S/D at 4 units per
second along each axis (D +x, A -x, S +z, W -z; diagonals combine).
Space makes it dash: it jumps 3 units at once in the direction it last
moved (+x if it hasn't moved yet; for diagonal movement, the dash goes
3 units along the diagonal). A dash can't happen within 2 seconds of the
previous one; a press during that time does nothing. Draw the player.
