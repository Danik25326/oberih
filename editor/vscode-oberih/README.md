# Oberih для VS Code

Підсвітка синтаксису та language server для `.obh`. Сервер — це сам `oberih` (`oberih lsp`),
Python не потрібен.

Можливості: діагностика (синтаксис і типи, із точними позиціями), форматування документа
(те саме, що `oberih fmt`), структура документа, перехід до визначення, hover із сигнатурою,
автодоповнення (ключові слова, типи, модифікатори, вбудовані функції, імена з файлу, поля
структур після `.`).

Межі: аналіз одного файлу (`import` не розгортається); немає rename/references/signature help.

## Збірка й встановлення

```
cd editor/vscode-oberih
npm install
npx @vscode/vsce package          # створить oberih-language-0.3.0.vsix
code --install-extension oberih-language-0.3.0.vsix
```

Якщо `oberih` не в PATH — задайте повний шлях у налаштуванні `oberih.serverPath`.

Граматику підсвітки (`syntaxes/oberih.tmLanguage.json`) згенеровано зі списків у
`src/lsp.rs`; тест `editor_grammar_covers_all_language_names` не дає їм розійтись.
