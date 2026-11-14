// Розширення VS Code для Oberih: підсвітка синтаксису + language server.
// Сервер — сам бінарник `oberih` у режимі `oberih lsp` (stdio, Content-Length), без Python.

const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function activate(context) {
  const serverPath = vscode.workspace.getConfiguration("oberih").get("serverPath", "oberih");

  const serverOptions = {
    command: serverPath,
    args: ["lsp"],
    transport: TransportKind.stdio,
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "oberih" }, { scheme: "untitled", language: "oberih" }],
  };

  client = new LanguageClient("oberihLanguageServer", "Oberih Language Server", serverOptions, clientOptions);

  client.start().catch((err) => {
    vscode.window.showErrorMessage(
      `Не вдалось запустити Oberih language server (\`${serverPath} lsp\`): ${err.message}. ` +
      "Перевірте, що `oberih` є в PATH, або задайте шлях у налаштуванні oberih.serverPath."
    );
  });
  context.subscriptions.push({ dispose: () => client && client.stop() });
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
