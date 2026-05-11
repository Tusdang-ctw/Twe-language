# Partner contribution guide — console targets + NDA-bound work

> Phase 40 session 5. Companion to
> [`docs/changes/2026-05-11-console-targets-rfc.md`](docs/changes/2026-05-11-console-targets-rfc.md).

This document is for **licensed indie studios** porting a Twe game to
Nintendo Switch, PlayStation, or Xbox. The open-source Twe repository
ships platform-agnostic abstractions; SDK-specific implementations
live in your private fork.

If you're an unlicensed developer interested in console ports, please
note: **Nintendo / Sony / Microsoft platform agreements prohibit the
public distribution of their SDKs**, signing keys, and store-API
client code. The path forward is to apply for a developer agreement
with the platform-holder you're interested in (links below).

---

## The partition

| Lives in **public `twec`** | Lives in **your private fork** |
|---|---|
| `console.controller(i)` abstract input layer | gilrs-replacement bindings to the platform input API |
| `console.glyph(button, style)` Unicode lookups | First-party glyph asset spritesheets (signed under SDK NDA) |
| `achievements.*` / `cloud_save.*` / `friends.*` trait stubs | Platform-specific implementations (NSO / Trophy / GamerScore) |
| Generic graphics abstractions (wgpu's HAL) | First-party graphics API backends (NVN / GNM / GDK D3D12) |
| `cargo build` working with no SDK present | Code-signing pipelines, cert generation |
| `examples/console_demo.twe` (no SDK calls) | Real shipping games using your platform SDK |

**The rule:** every file in the public repo must compile + run
without any platform SDK present. PRs adding SDK code get rejected.
The opposite direction — your private fork pulling in abstraction
changes from public `twec` — is always welcome.

## Setting up a partner fork

```sh
# 1. Fork twec on GitHub (private fork).
git clone https://github.com/<your-org>/twec-private.git
cd twec-private

# 2. Add the upstream remote so you can pull abstraction updates.
git remote add upstream https://github.com/Tusdang-ctw/Twe-language.git

# 3. Create a partner branch. Keep partner-private commits on it.
git checkout -b partner/<platform>-port

# 4. Add your platform SDK as a workspace member or as a feature-
#    gated dep. The flag mirrors `--features steam` / `--features
#    steam-net` in the public repo.
```

`Cargo.toml` example for a partner fork:

```toml
[features]
default = []
# Your platform feature. The flag controls every SDK call site.
nintendo-sdk = ["dep:nintendo-sdk-rs"]

[dependencies.nintendo-sdk-rs]
path = "vendor/nintendo-sdk-rs"   # NDA-bound; never committed to public
optional = true
```

## Where to wire platform-specific code

These are the canonical extension points the abstractions expose:

### Input

- Replace gilrs in `src/play.rs::poll_gamepad` with platform input
  bindings. Keep the same `gamepad` / `gamepad_axis` ambient shape
  so `console.controller(0)` reads from it unchanged.
- For multi-controller support (`console.controller(1)`, etc),
  extend `read_gamepad_buttons` / `read_gamepad_axes` in
  `src/stdlib.rs` to read from per-pad ambients
  (`gamepad_1` / `gamepad_2` / `gamepad_3`).

### Glyphs

- The public `console.glyph_asset(button, style)` returns
  `"glyph/<style>/<button>.png"`. Ship the signed glyph spritesheet
  in your private fork's `assets/glyph/<your-platform>/`.

### Achievements / cloud-save / friends

- Public `achievements.unlock(id)` routes through the Steam path on
  `--features steam`. Add platform-feature-flagged routes alongside:
  ```rust
  #[cfg(feature = "nintendo-sdk")]
  {
      nintendo_sdk::trophies::unlock(&id);
  }
  ```
- Public `cloud_save.save(slot, value)` + `.load(slot)` route the
  same way. Storage backends are platform-specific (NSO cloud, PSN
  storage, Xbox Live storage).
- Public `friends.list()` returns the platform-specific friend list
  via the partner-fork override.

### Graphics

- The wgpu backend is selected per-platform. Wire your platform's
  HAL into `src/play3d.rs::create_instance` behind your feature flag.
  wgpu's DX12 backend already covers Xbox Series X|S as of wgpu
  v22; PS5 GNM and Switch NVN require custom backends maintained
  by partner forks.

## Code-of-conduct + IP attribution

Partner contributions to the public repo follow the project's
existing CONTRIBUTING.md + CODE_OF_CONDUCT.md. Specifically:

- **Abstraction PRs (public).** Land under the project's MIT/Apache-2
  dual license once committed. Your studio's name appears in the
  PR's `Co-Authored-By` trailer if requested.
- **SDK code (private).** Stays in your fork. The project does not
  request, store, or distribute any SDK code, signing keys, or NDA-
  bound material.
- **Glyph assets (private).** Platform-owned glyph art is NDA-bound;
  ship it in your private fork's `assets/glyph/<platform>/`. The
  public repo's `console.glyph_asset` returns paths into that
  directory without bundling the assets themselves.

## Becoming a licensed developer

Out of scope for this document, but the canonical paths:

- **Nintendo Switch:** apply at `https://developer.nintendo.com`. The
  application process includes a project pitch and signed NDA.
- **PlayStation:** apply at `https://partners.playstation.net`. The
  process is similar.
- **Xbox:** the GDK (Game Development Kit) has two tiers — Microsoft
  Store ("Xbox Creators Collection") which is open to any developer,
  and "Xbox Series X|S Dev Kit" which requires a partner agreement.
  Start at `https://www.xbox.com/en-US/developers`.

## What this guide is *not*

- A how-to for platform certification. Each platform has its own TRC
  / TCR / XR checklist that changes per SDK release. Refer to the
  platform's own developer portal.
- A pricing or revenue guide. Out of scope.
- A list of currently-active partner studios. None exist at Phase 40
  closeout time (2026-05-11). The path is open; partners come or
  don't.
- A promise of support for SDK ports. The open-source `twec` ships
  the abstractions; partner forks own the per-platform work +
  per-platform maintenance.
