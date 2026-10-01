Each enemy is an instance of an entity class `Enemy` (give it a `pos`
field, a `vec3`, for drawing); remove expired enemies with `despawn`.
Keep the latest wave number in a top-level `var wave` (0 before the
first wave).
