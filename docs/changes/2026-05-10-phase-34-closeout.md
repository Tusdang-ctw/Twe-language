# Phase 34 closeout — Cross-platform polish

**Status:** codebase-closed 2026-05-10. Six sessions shipped in one batch (parallel-track scope: macOS focus / X11 focus / Wayland stub / cargo-dist matrix / cross-check CI gate / closeout).

**Phase numbering note.** The original gap-audit roadmap (mid-session edit on 2026-05-10) listed Phase 33 = "cross-platform polish" before Phase 33 (LLM differentiator) had been merged. The git tree resolved the collision in favor of the LLM differentiator commit landing first, so cross-platform polish moved to **Phase 34** in the canonical numbering. Both phases have closeout notes dated 2026-05-10.

This is the smallest of the round-2 phases. Closes the "macOS / Linux fully polished" row from the gap-audit table — the auto-pause-on-blur half via real platform code, the cargo-dist runtime cross-compile half via release-matrix expansion + a CI cross-check gate that catches breakage before tag-time.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | macOS focus path — `[[NSApplication sharedApplication] isActive]` via `objc2` `msg_send!` | `src/window_focus.rs`, `Cargo.toml` |
| 2 | Linux X11 focus path — parallel `x11rb` connection polling `_NET_ACTIVE_WINDOW` on root, then `_NET_WM_PID` on the active window, compared to `std::process::id()` | `src/window_focus.rs`, `Cargo.toml` |
| 3 | Linux Wayland focus path — documented stub returning `true`. Wayland focus is per-input-device and only delivered as events to the focused client; a separate Wayland connection (which we'd open here) would not be told who is focused. The honest stub stands until miniquad surfaces focus events upstream. | `src/window_focus.rs` |
| 4 | cargo-dist matrix expansion — `aarch64-unknown-linux-gnu` row in `release.yml`, cross-compiled on the x86_64 Linux runner via the `gcc-aarch64-linux-gnu` linker package | `.github/workflows/release.yml` |
| 5 | Cross-compile CI gate — new `cross-check` job in `ci.yml` runs `cargo check --release --target <T>` for `aarch64-unknown-linux-gnu` + `x86_64-pc-windows-gnu` on every PR | `.github/workflows/ci.yml` |
| 6 | This closeout note | `docs/changes/2026-05-10-phase-34-closeout.md` |

---

## What `is_focused()` looks like now

| Platform | Path | Status |
|----------|------|--------|
| Windows | `GetForegroundWindow` + `GetWindowThreadProcessId`, compare to `std::process::id()` | Phase 11 follow-on; unchanged |
| macOS | `objc2::class!(NSApplication).sharedApplication.isActive` via two `msg_send!` calls | **Phase 34 session 1** |
| Linux X11 | `x11rb` parallel connection → `_NET_ACTIVE_WINDOW` → `_NET_WM_PID` → compare to `std::process::id()` | **Phase 34 session 2** |
| Linux Wayland | Documented stub returning `true`; needs miniquad-upstream cooperation | **Phase 34 session 3** (honest deferral) |
| wasm32 | Returns `true` unconditionally; browser handles blur via page-visibility API at the macroquad WASM layer | Unchanged |
| Other (BSD / unknown) | Returns `true` unconditionally | Unchanged |

The `auto_pause_on_blur(true)` Twe builtin from Phase 11's follow-on now drives the pause flag correctly on Windows + macOS + X11 (XWayland sessions implicitly use the X11 path because `DISPLAY` is set inside Wayland sessions running XWayland). Pure-Wayland sessions stay un-paused on blur — the script can opt out via `auto_pause_on_blur(false)` if that becomes a UX issue.

---

## New crate dependencies

| Crate | Why | Where |
|-------|-----|-------|
| `objc2 = "0.5"` | macOS Objective-C runtime bridge for the `[NSApplication isActive]` query. Smallest dep that lets us send those two selectors safely. We do *not* pull `objc2-app-kit`; that crate is full AppKit bindings and we only need one selector. | `[target.'cfg(target_os = "macos")'.dependencies]` |
| `x11rb = "0.13"` | Pure-Rust X11 protocol implementation. Already pulled transitively via `arboard` (Phase 10 session 5b clipboard); we make the dep explicit so feature flags are stable. | `[target.'cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))'.dependencies]` |

Both deps are cfg-gated to their target platforms — Windows and WASM builds get neither in their dep tree.

`unsafe_code = "deny"` exception list (in `Cargo.toml`):

- `src/tagged_value.rs` — Phase 8.5 NaN tagging
- `src/window_focus.rs` — Win32 `GetForegroundWindow` (Phase 11) + macOS `msg_send!` (Phase 34)

No new files added to the unsafe-allow list.

---

## Cross-compile matrix after Phase 34

| Target | Host (release.yml) | Cross-check (ci.yml) | Status |
|--------|--------------------|----------------------|--------|
| `x86_64-pc-windows-msvc` | `windows-latest` | implicit (test job runs on Windows) | Phase 7 |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | implicit (test job runs on Linux) | Phase 7 |
| `x86_64-apple-darwin` | `macos-13` | (no Linux→macOS cross-check; build is the test) | Phase 7 |
| `aarch64-apple-darwin` | `macos-14` | (no Linux→macOS cross-check; build is the test) | Phase 7 |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` (cross-compiled with `gcc-aarch64-linux-gnu`) | yes — `cargo check` on every PR | **Phase 34 session 4** |
| `x86_64-pc-windows-gnu` | (not in release.yml; mingw cross-check only catches the build, not artifacts) | yes — `cargo check` on every PR | **Phase 34 session 5 (catches breakage early)** |

The Phase 34 cross-check job exists because tag-push is the wrong place to discover that someone broke a non-host target. A merged PR that fails cross-check is a normal CI failure that gets fixed before merge; a merged PR that breaks tag-time release is a hot-fix sprint.

---

## Honest deferrals

Reproduced from the Phase 34 plan as scoped in `docs/05-roadmap.md`:

1. **Linux Wayland focus detection.** Genuinely not solvable from outside the windowing-system client; the fix lives upstream in miniquad. If a Twe game running on a pure-Wayland session needs auto-pause-on-blur today, the workaround is `pause(true)` from a window-manager-aware shell script that `xdotool`-equivalents the focus event to a Twe builtin via stdin. Not a great workaround. Path forward: contribute focus-event surface to miniquad (likely 1–2 PRs), then this module's Wayland branch becomes a thin reader on those events.
2. **Pre-built binaries for `aarch64-pc-windows-msvc` (Surface Pro X / WoA laptops).** Possible but not on the gap-audit table; deferred until a community user asks.
3. **End-to-end auto-pause-on-blur smoke test on macOS / X11.** Phase 11's Windows path was tested manually by the implementer; the macOS / X11 branches were written on a Windows host and ride CI build validation but **were not smoke-tested live**. The `cargo check` cross-compile gate proves they compile; a real "alt-tab away, see the game pause" smoke test requires those two host machines and is a Phase 35 external-validation item (community contributor confirms behavior).

---

## Test deltas

| | Pre-Phase-34 | Post-Phase-34 |
|---|---|---|
| Lib unit tests | 534 | 535 (+1: `is_focused_returns_a_bool`) |
| Integration tests | 378 | 378 |
| **Total passing** | **912** | **913** |

The added test asserts the function is callable and returns a `bool`. The implementation can only be exercised against the running platform — there's no portable way to assert "yes the focus logic returned true because we have focus" inside a unit test (the test runner often has no window). Integration testing happens in the play loop's `BlurAutoPause` harness on each platform.

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean.

The 11 Windows-host CRLF-cascade failures from the Phase 33 closeout debrief reproduce on Windows working-directory checkouts; they are pre-existing line-ending issues unrelated to Phase 34's diff. Test count above measures isolated-run results, matching the Phase 33 closeout's methodology.

---

## Doc updates

- `docs/05-roadmap.md` — Round 2 section now lists Phases 33–41 (Phase 33 = LLM differentiator closed, Phase 34 = this phase, Phases 35–41 planned). Size table extended through Phase 41. v1.x scratch table re-pointed at the new phase numbers.
- `CLAUDE.md` — round-2 paragraph updated to reflect the renumbering (Phase 33 = LLM differentiator already closed, Phase 34 = cross-platform polish closed in this commit, Phases 35–41 planned).
- `Cargo.toml` — `unsafe_code` lint comment extended to mention `src/window_focus.rs` macOS Objective-C usage alongside the existing Win32 reference.
- README — no edits (test count is reported elsewhere; gallery + status snapshots already reflect post-Phase-32 state).

---

## What we learned

- **Wayland focus is genuinely architectural.** Three hours of looking for a "just query the compositor" path returned nothing portable. KDE Plasma exposes `org.kde.KWin` over D-Bus; GNOME exposes nothing equivalent; wlroots-based compositors expose `wlr-foreign-toplevel-management`. None work everywhere. The right answer is upstream miniquad work, not an `xdotool`-equivalent hack.
- **`x11rb` was already in our Cargo.lock.** Adding it explicitly added zero linker work — `arboard` already pulls it. Worth checking the transitive graph before assuming "new dep = bigger binary."
- **The cross-check job is cheap.** Running `cargo check` against two foreign targets in CI adds maybe 90 seconds to PR latency and catches the kind of regression that's expensive to discover on tag day.
