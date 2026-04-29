# Twe — VS Code extension

Syntax highlighting and language-server support for the [Twe
language](../../).

## What you get

- **Syntax highlighting** for `.twe` files via the TextMate
  grammar in [`syntaxes/twe.tmLanguage.json`](syntaxes/twe.tmLanguage.json).
  Highlights keywords, declarations, literals, operators, and
  embedded `{expr}` interpolation regions.
- **Diagnostics** — re-lex + re-parse on every keystroke, with
  squiggles at the line:col reported by `twec`'s parse errors.
  Backed by `twec lsp`, the in-process language server in
  [`src/lsp.rs`](../../src/lsp.rs).
- Auto-closing pairs for `()`, `[]`, `""`, `"""`. Comment
  toggling on `#`. Indent-on-enter after `:` headers.

Hover, go-to-definition, and completion are not in this MVP —
they ship in a follow-up session.

## Install (development)

The extension isn't published to the VS Code marketplace yet.
To run it locally:

```bash
cd editors/vscode
npm install                 # pulls vscode-languageclient
```

Then launch a VS Code "Extension Development Host" by opening
this directory in VS Code and pressing `F5`. Open any `.twe`
file in the new window and edits will get diagnostics live.

`twec` must be on your `PATH` (or you can set
`twe.serverPath` in settings to an absolute path; e.g.
`D:/IT/twe-language/target/release/twec.exe`).

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
