# Phase 10 closeout — UI + game-shell primitives (v0.4)

**Date:** 2026-05-04.
**Status:** closed.
**Roadmap reference:** `docs/05-roadmap.md` §"Phase 10".

---

## What shipped

Phase 10 ran in eleven sessions across 2026-05-04:

| # | Session | Surface |
|---|---------|---------|
| 1 | button | `button(at:, size:, label:) -> bool` immediate-mode primitive. |
| 2 | label / progress_bar | Stateless text + 0..1 fill widgets. |
| 3 | slider | Drag-state widget; one active slider tracked via `UI_STATE.active_slider`. |
| 4 | checkbox / dropdown | Toggle + selection widgets; dropdown carries open/closed state. |
| 5 | text_input | Click-to-focus; `get_char_pressed` drain + Backspace + blinking cursor. |
| 5b | clipboard | `os.clipboard.read/write` via `arboard`; Ctrl+V paste in `text_input`. |
| 6 | panel + stack + flex | Layout helpers returning `{at, size}` Objects. |
| 7 | grid + scroll | Grid cell layout + per-rect `scroll_y` driven by `mouse.wheel`. |
| 8 | pause | `pause(flag)` / `is_paused()`; play loop skips `tick_frame` while paused. |
| 9 | settings | `settings.set/get/has/set_default/save/load/try_load`; persist via the v0.2 save layer. |
| 10 | lang | `lang.set_locale/locale/load/t/tf`; JSON locale bundles; positional `{0}/{1}/...` substitution. |
| 11 | exit gate | `key_input` widget + `key_held(name)` / `key_pressed(name)` + `pause_menu_demo.twe` + `keybind_demo.twe`; `examples/survive.twe` rebound to read keys from `settings`. |

Two follow-on sessions also rolled into the same track:
- The if-expression form `let x = if cond: a else: b` parses now (closed the `examples/gamepad_demo.twe:9` latent bug). Documented in `docs/06-design-document.md` §3.
- A few small AST-shape tweaks (Tuple field accessors `.x` / `.y` / `.z` re-exercised by the new layout primitives).

**583 tests pass.** Clippy clean under `-D warnings`. `cargo build --release` zero warnings.

---

## Exit criteria

The roadmap's three Phase-10 exit-criterion bullets:

1. **A complete pause menu ships in `examples/`** — *met*. `examples/pause_menu_demo.twe` is a 156-line script that wires every Phase 10 primitive into a single demo: pause/resume, sliders, checkboxes, dropdown, text input, layout helpers, settings persistence, and localization (English + Japanese bundles).
2. **Settings round-trip across launches** — *met*. `settings.save("examples/pause_menu.save")` writes the data field via `save_to_path`; the next launch's `settings.try_load(...)` overlays it on top of `set_default`-seeded defaults. The 5 `settings_*` test cases in `tests/eval.rs` exercise the round-trip end-to-end.
3. **`examples/survive.twe` rebinds its keys at runtime via the settings UI** — *met*. `survive.twe` now seeds `settings.set_default("keys.right", "right")` (etc.), `settings.try_load`s `examples/survive.save`, and reads bindings via `key_held(settings.get("keys.right"))`. The companion `examples/keybind_demo.twe` ships a key-rebind UI built on the new `key_input` widget — click a row, press a key, click Save, and `survive.twe` picks up the new binding on next launch. Bundling the rebind UI inside `survive.twe` itself would have forced the vertical-slice game to grow a pause-menu state machine, which is a separate concern; the companion-script split keeps each demo's focus tight.

---

## What slipped

- **Auto-pause-on-window-blur.** Macroquad 0.4 doesn't expose focus events. Closing this needs a winit-integration session that swaps the play loop's input source. Captured as a follow-on, not blocking phase closure (the explicit `pause(flag)` / `is_paused()` primitives are sufficient for shipping pause menus; auto-blur is a polish item, not a load-bearing one).
- **Per-state pause opt-out** (`state foo: persistent` or `pause: false` syntax). Remains an open syntax question per `CLAUDE.md` "What is open". Deferred until a real game pressures it.
- **`save SaveSlot:` block syntax.** Still pending from v0.2; the bottom-layer `save_to` / `load_from` (and now `settings.save` / `load`) are the canonical persistence path. The block syntax is a v0.3+ ergonomics layer on top.
- **Localization plural rules + locale-aware number formatting.** Not in scope for Phase 10 — basic key→string lookup with positional placeholders is enough for shipping menus. ICU-style pluralization is a v1.x item.
- **Bytecode-VM mirror of the Phase 9 `on Class.death(e)` event hook.** Carried over; not a Phase 10 deliverable.

---

## Surface added

**Stdlib builtins (added in this phase):**

- Widgets: `button`, `label`, `progress_bar`, `slider`, `checkbox`, `dropdown`, `text_input`, `key_input`.
- Layout: `panel`, `stack`, `flex`, `grid`, `scroll`.
- Pause: `pause(flag)`, `is_paused()`.
- Input (dynamic-name): `key_held(name)`, `key_pressed(name)`.
- Clipboard: `os.clipboard.read()`, `os.clipboard.write(text)`.
- Settings: `settings.set/get/has/set_default/save/load/try_load`.
- Localization: `lang.set_locale/locale/load/t/tf`.

**Language:**

- If-expression: `if cond: a [elif d: e]* else: b` parses as an expression. Wired through eval, compiler, infer, printer, visual_check, ast_json.

**Internal infrastructure:**

- `UI_STATE` thread-local for stateful widgets, with rect-keyed identity (`RectId = (u64, u64, u64, u64)` via `f64::to_bits`).
- `clear_asset_caches` clears `UI_STATE` fields (slider, dropdown, text_input, key_input, scroll_y) on hot reload.
- Static `PAUSED: AtomicBool` for the pause flag; `crate::stdlib::is_paused()` is the play-loop hook.

---

## Files added

- `examples/pause_menu_demo.twe` — exit-gate demo, multi-locale.
- `examples/keybind_demo.twe` — key-rebind UI for `survive.twe`.
- `examples/pause_demo.twe` — minimal pause/resume demo (session 8).
- `examples/button_demo.twe` — minimal button demo (session 1).
- `examples/widgets_demo.twe` — full widget set demo (sessions 1–5).
- `examples/layout_demo.twe` — layout helpers demo (sessions 6 + 7).
- `examples/lang/en.json` — English locale bundle for `pause_menu_demo`.
- `examples/lang/ja.json` — Japanese locale bundle.

---

## Where Phase 10 lands the project

Codebase is now substantially beyond the original v0.1 surface. With Phase 8, 8.5, 9, and 10 all closed, the v0.1 release would carry v0.2 + v0.3 + v0.4 worth of features. Phase 7 release engineering (`cargo dist` binaries, VS Code marketplace publish, website, blog post) is now the only thing standing between the codebase and a public release; the version tag at release time will likely be v0.4 not v0.1.

The v1.0 thesis ("ship a Vampire-Survivors-class commercial 2D game on Twe") remains the prioritization filter. Phase 10 closes the load-bearing gap for shipping menus, settings persistence, and rebindable controls — all required surface for any Steam-class 2D game. Next on the critical path: Phase 11 (production hardening — crash reporter, screenshot, profiler, asset hot-reload reliability) per `docs/05-roadmap.md`.
