# Phase 39 closeout — Mobile (iOS / Android)

**Status:** codebase-scaffolding-closed **2026-05-11**. Nine sessions shipped: iOS + Android build-target layouts, touch input builtins, virtual joystick widget, signing docs, safe-area inset builtins, `examples/survive_beta_mobile/`, store-submission docs, and this closeout. Same shape as Phases 35 + 37 + 38: codebase work ships completely, store-submission gauntlet + signing pipeline + per-platform runtime hooks are explicit operator-action deferrals.

The technical work for mobile is bounded — macroquad already runs on both platforms natively. The real cost is store submission (App Store + Play Store review, content rating, signing). This phase ships everything Twe-side; what's left is operator-driven.

---

## Sessions shipped

| # | Surface | Files |
|---|---------|-------|
| 1 | `BuildTarget::IosAarch64` + `BuildTarget::AndroidAarch64` + layout writers | `src/build.rs` |
| 2 | `touch.*` input builtins (5 builtins) | `src/stdlib.rs` |
| 3 | `joystick(at:, size:, deadzone:)` widget | `src/stdlib.rs` |
| 4 | iOS signing recipe + Xcode integration docs | `docs/mobile-signing.md` |
| 5 | Android signing recipe + Gradle wrapper docs | `docs/mobile-signing.md` |
| 6 | `safe_area.*` inset builtins (5 builtins) + runtime setter `set_safe_area_insets` | `src/stdlib.rs` |
| 7 | `examples/survive_beta_mobile/{twe.toml, main.twe}` touch-controls reference scene | `examples/survive_beta_mobile/` |
| 8 | App Store + Play Store submission docs | `docs/mobile-signing.md` |
| 9 | Closeout + doc sync | this file, `docs/05-roadmap.md`, `CLAUDE.md`, `README.md` |

---

## What ships in detail

### Session 1 — iOS + Android build targets

`src/build.rs` gains two new `BuildTarget` variants:

- `IosAarch64` — parses `"ios"` / `"ios-aarch64"` / `"aarch64-apple-ios"`. Layout: `dist/<game>-ios/Payload/<game>.app/{Info.plist, <game>.twebundle}` + `README.txt`. The `Info.plist` template covers the App Store baseline — `CFBundleIdentifier` (defaults `dev.twe.<slug>`), `CFBundleExecutable`, `LSRequiresIPhoneOS`, `UILaunchStoryboardName`, `UIRequiredDeviceCapabilities = arm64 + metal`, landscape-only orientation. Producing a signed `.ipa` requires the cross-compile + codesign + zip recipe documented in `docs/mobile-signing.md` — operator-action.
- `AndroidAarch64` — parses `"android"` / `"android-aarch64"` / `"aarch64-linux-android"`. Layout: `dist/<game>-android/app/src/main/{AndroidManifest.xml, assets/<game>.twebundle}` + `README.txt`. The `AndroidManifest.xml` template references `android.app.NativeActivity` with `android.app.lib_name=twec`; the manifest's `package` attribute defaults to the Twe-slug-derived reverse-DNS. Producing a signed `.aab` requires the Gradle + NDK + keystore recipe in `docs/mobile-signing.md`.

Both targets produce zero new tests (consistent with Phase 38's "scaffolding-only deserves zero tests" stance) but verify clean via `cargo check --release` + `cargo clippy --release --all-targets -- -D warnings`.

### Session 2 — Touch input

`touch.*` namespace, 5 builtins:

- `touch.is_active()` — true iff any touch is currently active. On desktop returns false (no touch hardware).
- `touch.x()` / `touch.y()` — primary (first) touch position in screen pixels. Returns 0.0 if no touch.
- `touch.count()` — number of currently-active touches.
- `touch.pointer(i)` — returns `{x, y, id}` for the i-th touch, or nil if i ≥ count.
- `touch.tap_count()` — number of tap-release events in the last 500ms. Today returns 0 always; the play-loop tap-detection hook lands in the mobile-runtime follow-on session.

Implementation wraps macroquad's `touches() -> Vec<Touch>`, which exposes `id` + `position` per Touch entry. On desktop the vec is empty; on mobile/browser-with-touch it contains active touches across frames keyed by stable `id`.

### Session 3 — Virtual joystick widget

`joystick(at: (cx, cy), size: r, deadzone: d) -> {x, y, active, magnitude}`.

The widget *does no drawing of its own* — scripts compose the returned vector with whatever visual style they want. Behaviour:

- Finds the touch closest to `(cx, cy)` within `size` pixels (multi-touch stick selection — multiple `joystick(...)` calls in the same frame each pick their nearest active touch).
- Returns `active = false` + zero vector when no touch is within range.
- Returns `active = true` + zero vector when inside the deadzone (touching but not directing).
- Returns `active = true` + normalized direction + `magnitude` clamped to [0, 1] over the (deadzone, size] band when outside the deadzone.

`magnitude` is the band-relative distance, useful as a movement-speed multiplier — a half-pressed stick moves the player at half speed.

### Sessions 4 + 5 + 8 — Signing + store submission docs

`docs/mobile-signing.md` (~120 lines) ships the operator-action recipe for both platforms. Structure:

- **iOS path** — prerequisites (Xcode 15+, Apple Developer account, provisioning profile), 5 steps (`twec build` → cross-compile to `aarch64-apple-ios` → copy binary into Payload → `codesign` → zip into `.ipa`), known gotchas (Metal capability flag, landscape-only orientation, launch storyboard requirement).
- **Android path** — prerequisites (Android Studio / NDK r25 / Gradle 8, keystore, Play Console account), 5 steps (`twec build` → cross-compile to `aarch64-linux-android` → wrap with Gradle module → `./gradlew bundleRelease` → Play Console upload), known gotchas (NDK API level 21, keystore passwords, target SDK refresh deadline).
- **App Store / Play Store submission docs** — auth links to the canonical platform docs that change yearly (review guidelines, metadata requirements, content rating questionnaires, data safety form).

The doc is intentionally short. Annual-changing details point to the platform's own docs; the Twe-specific parts (which file goes where, which build flag matters) stay in the doc and update with the codebase.

### Session 6 — Safe-area insets

`safe_area.*` namespace, 5 builtins:

- `safe_area.top()` / `safe_area.bottom()` / `safe_area.left()` / `safe_area.right()` — pixel insets for each edge.
- `safe_area.rect()` — returns all four as a `{top, bottom, left, right}` record.

Plus a Rust-side public function `crate::stdlib::set_safe_area_insets(top, bottom, left, right)` that the mobile-runtime platform hooks invoke (iOS's `UIView.safeAreaInsets` observer; Android's `WindowInsets.systemBars` listener). Today every getter returns 0.0 — the platform hooks land in the mobile-runtime follow-on session.

Scripts written against these builtins keep working unchanged: today the HUD draws at `(0, 0)`, tomorrow (post-mobile-runtime) it draws at `(safe_left, safe_top)`. No script-side changes required.

### Session 7 — `examples/survive_beta_mobile/`

A two-file project (`twe.toml` + `main.twe`) demonstrating the Phase 39 mobile builtins composed into one scene:

- Virtual joystick anchored at the bottom-left safe-area corner. WASD fallback when no touch hardware.
- Player rectangle that the joystick / WASD moves.
- Multi-touch visualisation — yellow rings drawn at every active touch position.
- HUD inset by `safe_area.*` so it clears notches on iPhone-class devices.
- `assets.platform()` shown in the HUD so the demo proves out the cross-platform branching.

This is not a Vampire-Survivors port; the full content-effort port (enemies + bullets + level-up + waves) is tracked separately as a multi-session content effort. The demo is the canonical "Phase 39 builtins composed" reference scene — verify-clean, corpus-header-clean, runs on desktop + (once mobile-runtime lands) mobile.

### Session 9 — Closeout (this file)

Plus doc sync.

---

## API surface additions

Phase 39 adds **11 new builtins** + 2 new namespaces (`touch.*` and `safe_area.*`) + the `joystick(...)` top-level widget. Plus 1 new Rust-side public function (`set_safe_area_insets`) for the mobile-runtime hook.

| Namespace | Builtins |
|-----------|----------|
| `touch.*` | `is_active` / `x` / `y` / `count` / `pointer` / `tap_count` |
| `safe_area.*` | `top` / `bottom` / `left` / `right` / `rect` |
| top-level | `joystick` (widget) |

Combined with Phase 38's `assets.*` (3 builtins), the cross-platform-introspection + mobile-input surface is now 14 builtins. Build-target descriptors grew from 5 (post-Phase-36) to 8 (post-Phase-39): WindowsX86_64, MacOsAarch64, MacOsX86_64, LinuxX86_64, Wasm32, LinuxServer, Wasm32_3D, IosAarch64, AndroidAarch64.

---

## Test deltas

| | Pre-Phase-39 | Post-Phase-39 |
|---|---|---|
| Lib unit tests | 556 (post-Phase-38) | 556 (no new tests this phase — scaffolding-only) |
| Integration tests | 382 | 382 |
| **Total passing** | **938** | **938** |

Same pre-existing CRLF-cascade lib failures unchanged.

`cargo build --release` clean. `cargo clippy --release --all-targets -- -D warnings` clean after fixing one `doc_lazy_continuation` lint on the `AndroidAarch64` variant doc-comment.

The decision to ship zero new tests is consistent with Phase 38: scaffolding doesn't benefit from new unit tests. The next sessions to ship are the mobile-runtime platform hooks (iOS UIView observer + Android WindowInsets listener wiring `set_safe_area_insets`, plus the platform-specific cross-compile pipeline integration). Those land with smoke tests + signed artifact verification on real devices.

---

## Honest deferrals

The phase is *codebase-scaffolding-closed*. The following remain:

1. **Cross-compile pipeline integration.** `cargo build --target aarch64-apple-ios` + `aarch64-linux-android` works today via stock cargo; the integration with `twec build` (so a single command produces the signed artifact end-to-end) is a follow-on. Documented in `docs/mobile-signing.md` as a manual recipe meanwhile.
2. **iOS signing automation.** The `.ipa` packaging + `codesign` invocation + Transporter upload — operator-action today. CI integration via `cargo-dist` mobile target descriptors is a follow-on.
3. **Android signing automation.** Same shape as iOS — `./gradlew bundleRelease` is the operator step. CI integration via `cargo-dist` is a follow-on.
4. **Live platform safe-area + touch hooks.** `set_safe_area_insets` is wired and callable; the iOS `UIViewController.viewSafeAreaInsetsDidChange` observer + Android `WindowInsetsCompat` listener that *call* it land in the mobile-runtime follow-on.
5. **`touch.tap_count` play-loop hook.** Today returns 0; the play-loop tap-detection (sliding-window counter of tap-release events with <100ms hold duration) is a follow-on.
6. **Full `survive_beta` mobile port.** The reference demo `examples/survive_beta_mobile/main.twe` shows touch composition but isn't a full Vampire-Survivors port. The content port is tracked separately as a multi-session effort.
7. **TestFlight + internal-track Play Store playtest.** Operator action — `examples/survive_beta_mobile` running at 60fps on a 4-year-old phone is the exit-criterion measurement and requires real hardware.

---

## Doc updates

- `docs/05-roadmap.md` — Phase 39 entry updated to "codebase-scaffolding-closed 2026-05-11" with the 7 honest deferrals.
- `CLAUDE.md` — round-2 paragraph extended with Phase 39 closeout summary.
- `README.md` — test count unchanged at 938 (no new tests); examples gallery +1 (`survive_beta_mobile/`).

---

## What we learned

- **`macroquad`'s touch API is enough.** Wrapping `macroquad::input::touches()` gives the full multi-touch surface in ~80 LOC of stdlib glue. We didn't need a custom touch event-loop, didn't need to wrap UIKit / Android NDK touch APIs separately — macroquad already handles the platform-specific delivery.
- **Virtual joysticks are 60 LOC.** The `joystick(...)` widget is a small math function on top of the touch primitives. Deadzone + magnitude-band normalization is the whole logic. Scripts compose the visuals separately.
- **Safe-area-as-thread-local-Cell + late setter pattern works.** Scripts call `safe_area.top()` today and get 0.0 (correct on desktop); tomorrow the iOS / Android runtime hook fills the `Cell` and the same call returns the platform-correct inset. Zero script-side changes between today and the post-runtime-hook state.
- **Build-layout-only build targets are useful immediately.** Operators can wire CI pipelines + tooling against `--target ios` / `--target android` *today* — the layout is stable and the runner-side cross-compile is the operator's manual step. Same pattern Phase 38's `Wasm32_3D` used.
- **Cross-platform demos benefit from `assets.platform()` branching.** The `survive_beta_mobile` demo uses one source file across desktop (WASD fallback) and mobile (joystick). The branching is shallow — just "is touch active? if not, read keyboard" — but it makes the demo runnable on both targets today, which is what the operator wants for layout testing.
