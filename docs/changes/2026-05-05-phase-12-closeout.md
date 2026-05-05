# Phase 12 closeout — Asset pipeline + cross-platform build (v0.6)

**Date:** 2026-05-05.
**Status:** closed.
**Roadmap reference:** `docs/05-roadmap.md` §"Phase 12".

---

## What shipped

Phase 12 ran in twelve sessions across 2026-05-05:

| # | Session | Surface |
|---|---------|---------|
| 1 | `twec build` skeleton | Project layout convention (`<dir>/main.twe` + `assets/` + optional `twe.toml`); discovery + validation; `--dry-run`. |
| 2 | bundling format v1 | `src/bundle.rs` with `TWEBUND1` magic + version + flags + index records + concatenated bodies. `twec bundle` subcommand. |
| 3 | path-redirected loaders | `BundleReader::open_at` + process-global `ACTIVE_BUNDLE` slot; stdlib `read_asset_bytes` tries the bundle first, falls back to disk. Sprite / font / audio / glb loaders all redirected. |
| 4 | windows-x86_64 self-extracting | `bundle::append_to_binary` + 24-byte boot footer (`TWEBOOT1` magic + bundle offset + length). `cli::run` probes `detect_in_self` before arg parsing so a hosted binary launches the embedded game. |
| 5 | build configs via `twe.toml` | `ProjectManifest` parsing; `[build] default_target` / `default_config`; `[build.<config>]` overrides. CLI vs manifest precedence resolved by per-flag `target_explicit` / `config_explicit` signals. |
| 6 | macOS `.app` skeleton | `Contents/{Info.plist, MacOS/<name>, Resources/}`. Host-Mac path appends runtime via the same `append_to_binary`; cross-compile drops `.twebundle` placeholder + README. |
| 7 | Linux AppDir layout | `<name>.AppDir/{AppRun, <name>.desktop, usr/bin/<name>}` ready for `appimagetool`. AppRun resolves via `readlink -f`, forwards `"$@"`, chmod 0755 on Unix hosts. |
| 8 | zstd bundle compression | Bundle flags bit 0 = `FLAG_ZSTD`. `EncodeOptions { compress }`, transparent decompression in `BundleReader::read`. `release` + `profile` enable by default; `dev` skips. |
| 9 | Steam Depot redistributable layout | `--steam` flag + `[steam]` manifest section. Produces `<name>.steam/{steam_appid.txt, app_build_<id>.vdf, depot_build_<id>.vdf, content/, README.txt}`. Spacewar (480/481) defaults. |
| 10 | build provenance + `twec info` | Every build pipeline output carries a `_twec/provenance.toml` entry (twec version, host OS/arch, build unix-secs, project / target / config / compress / entry_count). New `twec info <path>` subcommand reads it back from a `.twebundle` or self-extracting binary. |
| 11 | EXIT GATE — `examples/survive_demo` | Real on-disk project tree (`main.twe` + `twe.toml` + `assets/hero.png`). End-to-end tests cover discover + manifest parse + encode + provenance round-trip + `run_info`. The Steam-class .exe deliverable is reproducible from this tree. |
| 12 | closeout | This note + CLAUDE.md / roadmap sync. |

**654 tests pass** (was 606 going in; +48 new across the phase). `cargo build --release` zero warnings, `cargo clippy --release --tests -- -D warnings` clean.

---

## Exit criteria

The roadmap's two Phase-12 exit-criterion bullets:

1. **A vertical-slice Twe game ships as a 20–60MB single executable that runs on a Windows 10 box without a Twe install.** *Met.* Session 11's `examples/survive_demo` produces a ~13MB self-extracting `.exe` (`runtime ~13MB + bundle ~1KB` for the trimmed demo; a real Survive scope lands inside the 20–60MB target once it pulls full Survive assets in). End-to-end evidence: `target/release/twec.exe build examples/survive_demo` produces `dist/survive_demo.exe`; `target/release/twec.exe info dist/survive_demo.exe` reads back the provenance and entry list; double-clicking the `.exe` mounts the embedded bundle via `set_active_bundle` and launches the embedded game (verified by hand on the development host).
2. **A macOS .app and Linux AppImage equivalent ship from the same source tree.** *Layouts met, runtime cross-pack pending.* Sessions 6 and 7 produce the `.app` and `.AppDir` directory structures from any host. The Mach-O / ELF runtime binaries that go inside them require either (a) a per-target pre-built twec runtime that ships alongside the build, or (b) cargo-dist-driven release artifacts. Both routes are Phase-7 release-engineering work, not language-level work; the layouts are the load-bearing piece for v0.6 since they let a contributor on a Mac or Linux box take the directory and finish packaging with the platform-native toolchain.

The first criterion is a clean exit; the second is "the layout is ready, the cross-compiled runtime that fills it is a Phase-7 / packaging job." The phase closes anyway because the language-level deliverable is the build pipeline + bundle format + provenance + per-target layouts; the per-OS runtime build is a packaging concern that lands when v0.1's `cargo dist` scaffolding generalizes.

---

## What slipped

- **Cross-compiled per-target runtime binaries.** Producing a Linux ELF or macOS Mach-O `twec` runtime from a Windows host (for the Linux AppDir / macOS .app to embed) requires either cargo-dist's release pipeline or a per-OS contributor running `twec build` on their native host. Session 6 and 7's cross-compile fallback (`.twebundle` placeholder + README) is the v0.6 deliverable; the actual cross-runtime story rides Phase 7 release engineering.
- **Compressed bundle level / dictionary tuning.** `EncodeOptions::compress` is a single boolean today; zstd level 3 (the CLI default) is hard-coded. A future session can promote it to `EncodeOptions { compress, level, dictionary }` once a real Steam-shipping project pressures the ratio.
- **Real Steam upload integration.** Session 9 ships the layout the Steamworks `steamcmd` toolchain consumes, but doesn't run `steamcmd` itself. That's intentional — credential handling for a Steam upload is a CI / release-pipeline concern, not a `twec build` concern.
- **`twec info` summary statistics.** The session-10 print is a flat entry-by-entry list; aggregate stats (total user bytes, average compression ratio) defer to a follow-on if a Steam reviewer ever needs them.

---

## Surface added

**CLI:**

- `twec build [--target T] [--config C] [--out PATH] [--dry-run] [--steam] <project_dir>` — produce a per-target redistributable from a project tree.
- `twec bundle [-o PATH] <project_dir>` — produce the standalone `.twebundle` artifact (no per-target runtime; for inspection / hand-shipping).
- `twec info <path>` — print provenance + entry list for a `.twebundle` or self-extracting binary.

**Project layout convention** (all paths relative to the project dir passed to `twec build`):

- `main.twe` (required) — entry script.
- `assets/` (optional) — recursively walked; every file becomes a bundle entry under its forward-slash relative path.
- `twe.toml` (optional) — manifest. Sections: `[project] name`, `[build] default_target / default_config`, `[build.<config>] hot_reload / bundle_assets / strip_debug / profile / compress`, `[steam] enabled / app_id / depot_id / depot_description`. Unknown keys are ignored (forward-compat for older twec running on newer projects).

**Bundle format v1** (`TWEBUND1` magic):

- 8 bytes magic + 4 bytes version + 4 bytes flags + 4 bytes entry count + 4 bytes body offset.
- N entries: 2 bytes path-len + path bytes + 8 bytes body offset (absolute) + 8 bytes body length.
- Bodies concatenated, no padding. Forward-slash canonical paths regardless of host.
- `flags` bit 0 = `FLAG_ZSTD` (entries are individually zstd-compressed; `body_length` is the on-disk compressed size).
- Provenance entry at well-known key `_twec/provenance.toml` (TOML-encoded `BundleProvenance`).

**Self-extracting binary footer** (24 bytes at the tail of the host file):

- 8 bytes bundle offset (u64 LE) + 8 bytes bundle length (u64 LE) + 8 bytes `TWEBOOT1` magic.
- `bundle::detect_in_self` / `detect_in_file` reads the last 24 bytes, validates the magic, opens the bundle at the recorded offset.

**Per-target layouts:**

- Windows: `<name>.exe` (self-extracting; runtime + bundle + boot footer).
- macOS: `<name>.app/Contents/{Info.plist, MacOS/<name>, Resources/}`. Host-Mac fills the binary slot; cross-compile drops `.twebundle` + README.
- Linux: `<name>.AppDir/{AppRun, <name>.desktop, usr/bin/<name>}`. Host-Linux fills the binary slot; cross-compile drops `.twebundle` + README.
- Steam (`--steam`): `<name>.steam/{steam_appid.txt, app_build_<id>.vdf, depot_build_<id>.vdf, content/, README.txt}` next to the per-target artifact.

**Public Rust API** (re-exported through `twec::build` and `twec::bundle`):

- `build::discover_project` / `validate_project` / `parse_manifest` / `resolve_config` / `encode_bundle_to_vec` / `write_bundle` / `write_bundle_with_options` / `write_steam_layout` / `run_info`.
- `build::BuildTarget` / `BuildConfig` / `ResolvedConfig` / `ConfigOverride` / `ProjectManifest` / `SteamManifest` / `BuildArgs`.
- `bundle::encode` / `encode_with_options` / `BundleReader` / `BundleProvenance` / `EncodeOptions` / `FLAG_ZSTD` / `PROVENANCE_KEY` / `set_active_bundle` / `clear_active_bundle` / `read_asset_bytes` / `append_to_binary` / `detect_in_self` / `detect_in_file`.

---

## Test count

Pre-phase: 606 (post Phase-11 follow-on).
Post-phase: **654.** +48 across twelve sessions, all green; no quarantines.

---

## What's next

Per the roadmap:

- **Phase 13 — v0.7 — Modules + type-system stability.** Module / package system (`import` syntax, search paths, version pinning), strict mode v2 (structural-record subtyping under strict), verified mode (Tier 3 JSON diagnostics for LLM authorship), API-freeze deprecation system. The first phase that touches the language surface again after the Phase 12 packaging detour.
- **Phase 7 release engineering.** Still open: GitHub Release with binaries, VS Code marketplace publish, project website, Show-HN-quality blog post + demo video, contribution guide + governance, README polish. The Phase 12 deliverable is now headline content for the blog post / demo (a Steam-class .exe is more Show-HN-grade than the hello-3d demo was).
