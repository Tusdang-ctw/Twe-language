// Minimal VS Code extension for Twe.
//
// Activates on the first `.twe` file the user opens; spawns
// `twec lsp` as a long-lived language server over stdio. The
// server's path is read from the `twe.serverPath` setting (default
// `twec`, resolved via PATH).
//
// Syntax highlighting is delivered by the TextMate grammar in
// `syntaxes/twe.tmLanguage.json` — that's enough for editor
// colouring without requiring tree-sitter inside VS Code (the
// tree-sitter grammar in `tree-sitter-twe/` is the one Neovim,
// Helix, and similar editors use).

const { LanguageClient, TransportKind } = require('vscode-languageclient/node');
const vscode = require('vscode');

let client;

function activate(context) {
  const config = vscode.workspace.getConfiguration('twe');
  const serverPath = config.get('serverPath') || 'twec';

  const serverOptions = {
    command: serverPath,
    args: ['lsp'],
    transport: TransportKind.stdio,
  };

  const clientOptions = {
    documentSelector: [{ scheme: 'file', language: 'twe' }],
    synchronize: {
      // Re-send every change to the server so it can re-publish
      // diagnostics. Matches the server's textDocumentSync = Full.
      configurationSection: 'twe',
    },
  };

  client = new LanguageClient(
    'twe',
    'Twe Language Server',
    serverOptions,
    clientOptions,
  );

  context.subscriptions.push(client.start());
}

function deactivate() {
  if (!client) {
    return undefined;
  }
  return client.stop();
}

module.exports = { activate, deactivate };
