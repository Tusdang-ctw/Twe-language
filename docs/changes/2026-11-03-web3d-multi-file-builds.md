# Multi-file games in built games, and `spawn module.Class` (Lanternfall E5)

**Date:** 2026-11-03
**Line:** Lanternfall engine enablers, E5 (second of E1–E6)
**Status:** shipped.

## Why

Lanternfall will be several thousand lines: player, weapons, bots, loot,
zone, HUD. One file that size is hard for a person to work in and hard for
an LLM to edit. `import` worked under `twec play3d`, but built games
ignored it:

- `twec build` packed only `main.twe` and `assets/` (`build.rs`: "Games
  are single-file today").
- The browser shell and a shipped `.exe` ran `main.twe` without the module
  loader.

So a multi-file game played on the author's machine and broke once
published. Two smaller gaps:

- `spawn` only took a bare class name, so `spawn bots.Bot` didn't parse.
- A module's top-level `spawn`, `scene` and `on update` / `on render` /
  `on X.death` ran in the module's own environment, which nothing ticks
  or draws. They silently did nothing (against Principle 3).

## Decision: bundle the modules, don't concatenate

Concatenating every file into one before building would have been
smaller. It would also have dropped module scoping: `brain.think`
qualification, functions resolving free names in their own module
(docs/06 §9.1), and name clashes between files. A built game would then
behave differently from the same game under `twec play3d`. So the build
packs each module as its own bundle entry and the runtime loads them as
modules.

## What ships

- **The build packs modules.**
  - `build::bundle_entries` writes `main.twe`, then every transitively
    imported module, keyed by its path relative to the project folder
    (`ai/brain.twe`), then `assets/`.
  - It is the one list both bundle encoders use; there used to be two
    copies.
  - `validate_project` loads the whole import graph, so a missing module
    fails the build at the `import` that names it, with file, line and
    column. A module outside the project folder is a build error.
- **Built games load modules from the bundle.**
  - `module::prepare_entry_bundled(src, init)` loads the graph with
    `BUNDLED` set: paths are bundle keys, existence is
    `bundle::bundle_has`, and reads come from the bundle with no
    filesystem fallback.
  - `frame2d::load_source_any` (the shipped `.exe`) and the browser shell
    call it when `main.twe` imports anything.
  - `init` runs on every module's environment before its top level, which
    is how the browser turns on GPU particles for the whole game.
  - Resolution is the same rule as at dev time (relative to the importing
    file), so a game resolves its imports identically in both.
- **`spawn module.Class`.**
  - The parser stores the dotted path as the class name. The printer and
    the AST JSON round-trip it unchanged.
  - Eval looks up the module, then the class (`lookup_spawn_class`).
  - Updated: the GBNF export (`'spawn' IDENT ('.' IDENT)?`) and
    tree-sitter (optional `module:` field, plus a corpus case).
- **Modules can't touch the world.** After a module's top level runs,
  `module::refuse_module_world` errors if it spawned an entity, declared a
  scene, or registered a top-level or death handler. The help text says to
  keep classes and functions in modules and spawn from the main file.

## Not included

- `twe.toml` `[dependencies]` and search paths are still only consulted by
  `twec run <dir>`. Built games, like `twec play3d`, resolve imports
  relative to the importing file.
- `twec verify` still checks one file at a time.

## Tests

- **`tests/build.rs`:**
  - `multi_file_game_runs_from_its_bundle`: three modules, including a
    nested import next to its importer. The project folder is deleted
    before loading, so everything comes from the bundle. Output `42\n1`
    proves the module functions ran and the spawned entity exists.
  - `a_missing_import_fails_the_build_where_it_is_named` (`main.twe:2:1`).
  - `a_module_that_spawns_at_its_top_level_is_refused`.
- **`tests/fmt.rs::spawn_of_a_module_class_round_trips`.**
- **Tree-sitter corpus** "spawn a module's class" (35/35 parse).

## Docs

`docs/06` §3.6 (`spawn_stmt`), §7.7 (`spawn bots.Bot`), §9.2 / §9.3
(rewritten to match the implemented `import "path" [as name]` and to
cover built games), and the CHANGELOG.
