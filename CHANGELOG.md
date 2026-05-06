# Changelog

The deprecation log for the public Twe surface. See
`docs/05-roadmap.md` for the phase-by-phase development log; this
file is the user-facing record of what changed between releases.

The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/) once v1.0
ships. Until then, every minor (v0.x) release is permitted to break
the surface, with deprecations rather than removals where the
removal would be load-bearing.

## v0.7 (Phase 13) — Modules + type-system stability

**Status:** in development.

This is the public-API freeze that v0.8+ depends on. Anything
flagged with `@deprecated("since v0.7")` here will keep working in
v0.7.x and v0.8 (a 12-month carry-over per
`docs/05-roadmap.md` §"Phase 13"), then be removed in v1.0.

### Added

- **Module / package system.** `import "<path>"` and
  `import "<path>" as Alias` bind a module value whose fields are
  the imported file's top-level names. Multi-file projects are
  supported out of the box; the importer's directory is the
  default search path.
- **`twe.toml [dependencies]`.** Each entry maps a logical name to
  a search path (table form) or a version pin (string form). The
  resolver consults dependency paths before the importer's directory.
- **Strict mode v2.** Structural-record subtyping (`{x: int, y: int}`)
  and Luau-style lax narrowing (a Union → variant assignment is
  accepted as an implicit narrowing assertion).
- **Verified mode (Tier 3).** `# verified` directive + the
  `twec verify <file>` subcommand emit a JSON document an LLM can
  sit in a self-correction loop with.
- **`@deprecated("since vX.Y")` annotations.** Attach to top-level
  function and type declarations. `twec verify --warn-deprecated`
  surfaces a `deprecation` warning per use site.

### Deprecated (since v0.7)

(None yet — first cycle. As the surface evolves through v0.7.x and
into v0.8, additions here document the 12-month-carry-over schedule
each retired symbol is on.)

### Changed

- The `# strict` directive's behavior: structural records and
  Union-to-variant lax narrowing are now part of the strict
  contract. Programs that relied on strict rejecting these will
  see fewer diagnostics. No source-level breakage — the change is
  purely "fewer errors in strict mode."

### Removed

(None.)

---

Earlier phases (v0.1 — v0.6) are tracked in
`docs/changes/` as per-session closeout notes; this file picks up at
v0.7 because the API-freeze contract is what users care about, and
that's a Phase 13 concern.
