# Doc 07 — Save System Design

> Design doc for the `save` block compiler shipping in v0.2 (Phase 8 of `docs/05-roadmap.md`). Authored as a Phase 7 prerequisite — the v0.2 `save` implementation must conform to the model laid out here.
>
> **Status:** design phase. No code yet. Implementation follows v0.1 release.

---

## Why a design doc

Save systems are usually retrofit. The first version is "throw a struct at JSON.stringify"; the second version is "now versioned" once the first save format breaks; the third version is "now Steam Cloud–compatible" once the storefront cares. By the third version the save code is the most-bug-prone part of the codebase.

Twe makes saves a language construct (`save` block, Example 7 in `docs/01-examples.md`). That gives the compiler a single place to enforce: schema integrity, migration ordering, atomicity, storage-backend abstraction, and Steam-Cloud compatibility — all of which are decisions, not features. Decide once here.

---

## What's locked from Example 7

The `save` block grammar is fixed by Example 7. Re-stating it for clarity:

```twe
save SaveSlot:
    version: 2
    player:
        pos: vector
        hp: int
        inventory: list of Item
    world:
        seed: int
        time_of_day: time

    migrate from version 1:
        # body runs against `old` (the previously-decoded value)
        player.hp = old.player.health
```

Locked semantics:

- A `save` block is a *typed schema*, not a free-form record.
- `version: N` is required. Integer monotonically increasing.
- Nested fields are namespaces (`player`, `world`); their bodies are typed records.
- `migrate from version N:` blocks are first-class. The compiler runs them in version order on load.
- Block-literal construction: `SaveSlot { player: { ... }, world: { ... } }`.
- Built-in builtins: `save_to(path, as: SaveSlot { ... })` and `load_from(path, as: SaveSlot) -> SaveSlot`.

---

## Design decisions for v0.2

### 1. Storage format

**Decision: a versioned binary format with a JSON header for diagnostics.**

Each save file:

```
[16-byte magic + format-version header]
[u32 schema version]
[u32 length-prefixed JSON metadata: { schema: "SaveSlot", twe_version: "0.2.x", saved_at: ISO-8601 }]
[binary payload — bincode-encoded values]
[8-byte CRC-64 of the preceding bytes]
```

Why not pure JSON? Saves carry binary blobs (textures, audio handles, large lists). JSON-encoding those is wasteful and slow. Why include a JSON header? Lets `twec save inspect <file>` show the schema + version + timestamp without decoding the payload — useful for debugging and Steam Cloud's web inspector.

The CRC catches truncation and storage corruption. Recovery on CRC mismatch is application-defined (most games will offer "load backup" or "start new game").

### 2. Atomicity

**Decision: write-to-temp + rename + fsync, with an explicit autosave rotation slot.**

`save_to(path, as: ...)` semantics:

1. Write to `path.tmp`.
2. `fsync` the temp file.
3. Rename `path.tmp` → `path` (atomic on POSIX and modern Windows NTFS via `MoveFileEx(MOVEFILE_REPLACE_EXISTING)`).
4. `fsync` the parent directory.

Failure between (1) and (3) leaves the previous `path` intact; failure between (3) and (4) leaves the new `path` correct but possibly not flushed (rare). Either case: no half-written save.

Autosave rotation is a separate API: `save_rotating(slot, n: 3, as: ...)` keeps the last 3 saves under `slot.save`, `slot.save.1`, `slot.save.2`, rotating on each write. Recovery from a corrupt slot reads the next-newer `slot.save.K`. Optional; games that don't want it use plain `save_to`.

### 3. Migration semantics

**Decision: linear migration chain, run in version order. Each `migrate from version N:` block transforms the previous version's decoded value into the next.**

Algorithm on `load_from`:

1. Read header. Note the on-disk version (`V_disk`).
2. Decode the payload into the *V_disk-shaped* type (the compiler keeps the historical schemas — see §4).
3. For each `migrate from version K:` block where `K >= V_disk`, run the block in order. Each block sees `old` (the previous version's value) and produces the next version's value.
4. After all migrations, return the value as the *current* schema type.

The `old` binding is read-only; the migration body assigns to the *new* schema's fields directly (block-literal-style). Compile-time check: every field in the new schema must be either copied from `old`, defaulted, or explicitly computed. No silent zero-fills.

If `V_disk > V_current`, error: "save was written by a newer version of the game." No backwards migrations.

### 4. Historical schemas

**Decision: the compiler retains every historical schema as a separate type for decode purposes.**

`save SaveSlot:` with `version: 2` and a `migrate from version 1` block implies *two* schema types exist:
- `SaveSlot__v1` (the schema as it existed at version 1, reconstructed by reverse-applying the migration's diff).
- `SaveSlot__v2` (the current schema, the one the user writes).

The compiler infers `SaveSlot__v1` from the migration block (which fields it reads from `old`, plus any explicit field declarations the migration retains). For schemas that diverge significantly across versions, the user can `save SaveSlot__v1:` explicitly as a separate block referenced by the migration.

Open question: how to handle a schema rename (`save Player → save HeroSave`). Probably `save HeroSave: was Player`. Defer until needed.

### 5. Storage backend abstraction

**Decision: pluggable backends with the filesystem default; Steam Cloud is a v0.9 plug-in.**

The `save_to` / `load_from` API is backend-agnostic. The default backend is the filesystem under a per-platform game-saves directory:

| Platform | Default save directory |
|---|---|
| Windows | `%APPDATA%/<game>/saves/` (more specifically `%APPDATA%/Roaming/<game>/saves/`) |
| macOS | `~/Library/Application Support/<game>/saves/` |
| Linux | `$XDG_DATA_HOME/<game>/saves/` (default `~/.local/share/<game>/saves/`) |

`<game>` is set by the game's manifest (TBD: `game.toml` or similar; locked in Phase 12 alongside the build pipeline).

Per-game override via `os.set_save_root(path)` (rare; mostly for testing).

### 6. Steam Cloud compatibility

**Decision: design for it now; ship the integration in Phase 15 (v0.9). Don't retrofit.**

Steam Cloud constraints (per Steam's docs as of 2025):

- Per-file size limit: configurable per app (default 100 MB).
- Total quota: configurable per app (default 1 GB).
- File counts: low thousands per slot, not millions.
- Sync semantics: file-level last-writer-wins; conflict resolution is opt-in via the SDK's `Get`/`Put` API rather than transparent.
- Path mapping: the SDK redirects writes from a designated *root* (typically `%APPDATA%/<game>/`) to the cloud.

Implications for v0.2 design:

- **Save files should be small.** Encourage games to split state across multiple `save` blocks rather than one mega-save. Compiler should warn on a `save_to` call that produces > 10 MB — almost certainly a bug.
- **Files should be in the platform save root.** Don't write to arbitrary paths the cloud sync ignores.
- **No per-byte streaming.** Each `save_to` is one full write. Don't hold open file handles between writes.
- **Conflict resolution is the game's responsibility.** Twe surfaces the conflict via a return value from `load_from` (an enum: `Loaded`, `LoadedFromBackup`, `Conflict { local: SaveSlot, cloud: SaveSlot }`). The game decides.

The actual SDK call (`SteamRemoteStorage_FileWrite` / `_FileRead`) lands in Phase 15 as a backend swap. Same `save_to` API, different storage layer.

### 7. Privacy + security

**Decision: saves are user data. No telemetry, no analytics, no upload without explicit consent.**

- Save files are written readable to the local user only (`0600` on POSIX; Windows ACLs default to user-only). No world-readable saves.
- No saved telemetry hooks. The crash-reporter (Phase 11) is opt-in and separate from save data.
- Save encryption: not in v0.2. If a game wants encrypted saves (anti-cheat for leaderboards, etc.), provide via `save_to(path, as: ..., encrypt_with: key)` in a later phase. Not v1.0 critical.

---

## What v0.2 implements

A minimal-viable subset:

- `save` block parser + AST.
- Schema-type generation in the inferer (current + historical via migration analysis).
- `save_to(path, as: SchemaName { ... })` builtin: write-to-temp + atomic rename, with the binary format from §1.
- `load_from(path, as: SchemaName) -> SchemaName` builtin: read, CRC-check, decode, run migrations, return.
- Filesystem backend only (Steam Cloud rides Phase 15).
- Tests covering: round-trip, version migration (v1 → v2), corrupt-CRC error, missing-file error, newer-version error.

What v0.2 explicitly does NOT do:

- Steam Cloud integration (Phase 15).
- Encrypted saves.
- Save-file UI inspector (`twec save inspect <file>`) — that's a tooling-polish item, Phase 11 or later.
- Schema renames (`save HeroSave: was Player`).
- Autosave rotation (lands in Phase 10's settings system or Phase 11's hardening).

---

## Open questions

1. **Backwards-migration support.** Currently the design forbids `V_disk > V_current` ("save from a newer version"). Should games be allowed to declare `migrate to version N` blocks for forward-compat? Probably no — the upgrade path is "ship a patch that bumps the schema." Defer indefinitely.
2. **Cross-game save sharing.** A New Game Plus that consumes saves from a previous title. Out of scope for v1.0 — too game-specific. Could add via `os.import_save(path, as: ForeignSchema)` later.
3. **Time-of-day type's serialization.** Example 7 uses `time_of_day: time`. The `time` type isn't pinned in `docs/06-design-document.md` — pick a representation (UTC seconds since epoch as i64? in-game-day fraction as f64?) before the v0.2 implementation lands.
4. **Where does `game.toml` (manifest) live?** Phase 12 owns this, but the save-root-derived directory needs a stable game identifier from day one of v0.2. Tentative: read from `Cargo.toml`-style `[twe.game]` metadata on the project root. Confirm in Phase 12.

---

## References

- Example 7 in `docs/01-examples.md` — locked grammar.
- `docs/05-roadmap.md` Phase 8 — implementation slot.
- *Crafting Interpreters* §22 (Hash tables) and §28 (Methods and Initializers) — schema-type generation patterns.
- Steam Cloud documentation: https://partner.steamgames.com/doc/features/cloud (consulted for §6 constraints; URL provided by user-context, do not rely on WebFetch).
- POSIX `rename(2)` and Windows `MoveFileExW` semantics — atomic rename guarantees.
- `bincode` 2.x crate — likely binary-format dependency for §1's payload (justified per `CLAUDE.md`'s "every new crate requires justification" policy when v0.2 implementation actually adds it).
