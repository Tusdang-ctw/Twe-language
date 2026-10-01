A conversation moves between three nodes, chosen with A and B:

- "start": A goes to "shop", B goes to "quest".
- "shop": A buys a potion for 10 gold (only if there are at least 10
  gold; either way it stays at "shop"); B goes back to "start".
- "quest": A accepts the quest (it is now taken) and goes back to
  "start"; B goes back to "start" without it.

It begins at "start" with 25 gold, the quest not taken and no potions.
