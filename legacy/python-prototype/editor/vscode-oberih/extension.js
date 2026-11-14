// Точка входу VS Code розширення Oberih.
// Запускає lsp_server.py як окремий процес і підключає стандартний
// vscode-languageclient для комунікації через stdio (той самий протокол,
// що перевірений у tests/run_example_lsp.py).

const path = require("path");
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function activate(context) {
  const config = vscode.workspace.getConfiguration("oberih");
  const pythonPath = config.get("pythonPath", "python3");
  const serverPath = path.join(context.extensionPath, "..", "..", "lsp_server.py");

  const serverOptions = {
    command: pythonPath,
    args: [serverPath],
    transport: TransportKind.stdio,
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "oberih" }],
  };

  client = new LanguageClient(
    "oberihLanguageServer",
    "Oberih Language Server",
    serverOptions,
    clientOptions
  );

  client.start();
  context.subscriptions.push(client);
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
