An inventory has 4 slots, empty at the start. Pressing A, B or C picks
up one item of that kind ("a", "b" or "c"). A picked-up item goes onto
the first slot (lowest index) that holds that kind and has fewer than 3
items; if there is none, it goes into the first empty slot; if no slot
is empty, it is not picked up and the count of rejected items goes up by
1. Pressing X drops the whole stack in the last non-empty slot (highest
index), leaving that slot empty. Slots don't move when another slot is
emptied.
