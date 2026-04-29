# Twe — VS Code extension

Syntax highlighting and language-server support for the [Twe
language](../../). Twe is a game-first scripting language with
first-class state machines, scenes, dialogue, and a wgpu-driven
3D backend; this extension is the editor side of that.

## What you get

- **Syntax highlighting** for `.twe` files via the TextMate
  grammar in [`syntaxes/twe.tmLanguage.json`](syntaxes/twe.tmLanguage.json).
  Highlights keywords, declarations, literals, operators, and
  embedded `{expr}` interpolation regions.
- **Diagnostics** — re-lex + re-parse on every keystroke, with
  squiggles at the line:col reported by `twec`'s parse errors.
  Strict-mode type errors (when the file opts in via a
  `# strict` directive) surface inline as well.
- **Hover** shows the inferred type for any identifier under the
  cursor. Lets / vars / functions / methods / fields are all
  resolved.
- **Go-to-definition** (`F12`) jumps to the source of the
  identifier under the cursor — top-level decls, methods, states,
  fields.
- **Completion** offers user-declared symbols (with their
  inferred type as the detail), Twe keywords, and stdlib
  namespaces (both bare `math` and dotted `math.abs`).
- Auto-closing pairs for `()`, `[]`, `""`, `"""`. Comment
  toggling on `#`. Indent-on-enter after `:` headers.

Backed by `twec lsp`, the in-process language server in
[`src/lsp.rs`](../../src/lsp.rs).

## Install (development)

The extension is not yet published to the VS Code marketplace —
publishing rides the v0.1 release cut. To run it from source:

```bash
cd editors/vscode
npm install                 # pulls vscode-languageclient
```

Then launch a VS Code "Extension Development Host" by opening
this directory in VS Code and pressing `F5`. Open any `.twe`
file in the new window and edits will get live diagnostics,
hover types, completion, and go-to-definition.

`twec` must be on your `PATH` (or you can set
`twe.serverPath` in settings to an absolute path; e.g.
`D:/IT/twe-language/target/release/twec.exe`).

## Install (packaged `.vsix`)

Once `vsce package` runs cleanly (see "Publishing" below), you
can sideload the resulting `.vsix` into any VS Code:

```
code --install-extension twe-language-0.1.0-pre.vsix
```

This is the recommended path for users who don't want to run
from a checkout.

## Publishing

Marketplace publishing is gated on the v0.1 release per the
roadmap. The mechanics:

```bash
npm install -g @vscode/vsce          # one-time
cd editors/vscode

# Verify the package builds cleanly. This produces a `.vsix`
# in the current directory.
vsce package

# Once a publisher account exists at marketplace.visualstudio.com:
vsce login twe-lang
vsce publish
```

The `publisher` field in `package.json` (`twe-lang`) must match a
real Marketplace publisher you control. The README, repository,
homepage, license, and keyword fields all flow into the listing
page — keep them honest.

`.vscodeignore` (sibling file) lists the paths excluded from the
published package: `node_modules/` is excluded because the
runtime install pulls dependencies fresh; the LSP itself is
expected on the user's PATH (the extension does not bundle
`twec`).

## How it talks to twec

Activation triggers on the first `.twe` file open. The extension
spawns `twec lsp` as a long-lived child process and pipes LSP
messages over stdio. The server is single-threaded and stateless
between sessions; killing the editor kills the server.

## Settings

| key | default | purpose |
|---|---|---|
| `twe.serverPath` | `twec` | Where to find the `twec` binary. Set to an absolute path if not on PATH. |
| `twe.trace.server` | `off` | LSP trace level. Set to `verbose` to debug server / client communication in the Output panel. |

## Roadmap

- **Marketplace publish** — gated on v0.1 release.
- **Code actions** for the `did_you_mean` suggestions surfaced by
  strict-mode diagnostics — turn the help text into a click-to-fix.
- **Inlay hints** for inferred types on `let` / `var` bindings,
  matching the hover output but always visible.
- **Semantic tokens** for richer highlighting than TextMate (e.g.
  distinguish a state name from an entity name).

These are post-v0.1 — open issues on the repo before adopting.
