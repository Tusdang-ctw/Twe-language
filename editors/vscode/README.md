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

## Install

From the VS Code Marketplace:

```
code --install-extension twena-inc.twe-language
```

Or open the Extensions panel (`Ctrl+Shift+X`), search for **Twe Language**, click **Install**.

You also need `twec` on your `PATH` — the extension drives diagnostics, hover, and go-to-definition by spawning `twec lsp` as an LSP child process. Two ways to get it:

- **Download a release binary** from the [GitHub Releases page](https://github.com/Tusdang-ctw/Twe-language/releases) and put it on your `PATH`.
- **Build from source:** `cargo install --path .` from the repo root.

If `twec` lives somewhere off-`PATH`, set `twe.serverPath` in VS Code settings to its absolute path (e.g. `D:/IT/twe-language/target/release/twec.exe`).

## Install (development, from a source checkout)

To run the extension straight from this directory instead of the published package:

```bash
cd editors/vscode
npm install                 # pulls vscode-languageclient
```

Then open this directory in VS Code and press `F5` — that launches an "Extension Development Host" window. Open any `.twe` file in the new window and edits will get live diagnostics, hover types, completion, and go-to-definition.

## Publishing (maintainers)

The extension is published as `twena-inc.twe-language`. To cut a new release:

```powershell
npm install -g @vscode/vsce          # one-time
cd editors/vscode

# Bump version in package.json (Marketplace rejects re-publishing the same version).

# Verify the package builds cleanly. Produces a .vsix in the current directory.
vsce package

# Publish using a Personal Access Token (env var or -p flag):
vsce publish -p $env:VSCE_PAT
```

The PAT is an Azure DevOps token created at `https://dev.azure.com/<org>/_usersSettings/tokens` with **All accessible organizations** + **Marketplace → Manage** scope.

`.vscodeignore` (sibling file) lists paths excluded from the published package: `node_modules/` is excluded because the runtime install pulls dependencies fresh; the LSP itself is expected on the user's `PATH` (the extension does not bundle `twec`).

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

- **Code actions** for the `did_you_mean` suggestions surfaced by
  strict-mode diagnostics — turn the help text into a click-to-fix.
- **Inlay hints** for inferred types on `let` / `var` bindings,
  matching the hover output but always visible.
- **Semantic tokens** for richer highlighting than TextMate (e.g.
  distinguish a state name from an entity name).

These are post-v0.1 — open issues on the repo before adopting.
