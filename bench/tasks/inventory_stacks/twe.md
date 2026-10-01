Keep the slots in a top-level `var slots`: a list of 4 entries, each
either `nil` (empty) or a tuple `(kind, count)`, e.g. `("a", 2)`. Keep the
number of rejected items in a top-level `var rejected`.
