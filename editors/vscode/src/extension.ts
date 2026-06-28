// fm-bridge VS Code extension entry point.
//
// Wires up four features over the fm-bridge Rust binary:
//   - Read script from clipboard  → opens the decoded .fmscript
//   - Write script to clipboard   → encodes the active .fmscript for FileMaker
//   - Diagnostics                 → underlines format errors (on type + on save)
//   - Autocomplete                → step names from the binary's catalog
//
// All FileMaker know-how lives in the binary; this file is glue.

import * as vscode from "vscode";
import {
  BinaryNotFoundError,
  parseScript,
  readClipboard,
  resetBinaryCache,
  writeClipboard,
} from "./bridge";
import { StepCompletionProvider, resetCatalogCache } from "./completion";

const LANGUAGE = "fmscript";

export function activate(context: vscode.ExtensionContext): void {
  const diagnostics = vscode.languages.createDiagnosticCollection(LANGUAGE);
  context.subscriptions.push(diagnostics);

  context.subscriptions.push(
    vscode.commands.registerCommand(
      "fm-bridge.readFromClipboard",
      readFromClipboard,
    ),
    vscode.commands.registerCommand(
      "fm-bridge.writeToClipboard",
      writeToClipboard,
    ),
    vscode.languages.registerCompletionItemProvider(
      LANGUAGE,
      new StepCompletionProvider(),
    ),
  );

  registerDiagnostics(context, diagnostics);

  // The configured binary path may change which binary (and catalog) we use.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("fmBridge.binaryPath")) {
        resetBinaryCache();
        resetCatalogCache();
      }
    }),
  );
}

export function deactivate(): void {
  /* nothing to clean up beyond context.subscriptions */
}

// ─── Commands ───

async function readFromClipboard(): Promise<void> {
  try {
    const resp = await readClipboard();
    if (resp.status !== "ok" || resp.script_text === undefined) {
      void vscode.window.showErrorMessage(
        `fm-bridge: ${resp.error ?? "could not read clipboard"}`,
      );
      return;
    }
    const doc = await vscode.workspace.openTextDocument({
      language: LANGUAGE,
      content: resp.script_text,
    });
    await vscode.window.showTextDocument(doc);
  } catch (err) {
    reportError(err);
  }
}

async function writeToClipboard(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    void vscode.window.showErrorMessage(
      "fm-bridge: open a .fmscript file first.",
    );
    return;
  }
  try {
    const resp = await writeClipboard(editor.document.getText());
    if (resp.status === "ok") {
      void vscode.window.showInformationMessage(
        "fm-bridge: script copied — paste it in FileMaker (Cmd/Ctrl+V).",
      );
      return;
    }
    await showWriteError(editor, resp.error, resp.error_line);
  } catch (err) {
    reportError(err);
  }
}

/** Show a write/parse error and offer to jump to the offending line. */
async function showWriteError(
  editor: vscode.TextEditor,
  message: string | undefined,
  line: number | undefined,
): Promise<void> {
  const text = `fm-bridge: ${message ?? "could not write to clipboard"}`;
  if (line && line > 0) {
    const choice = await vscode.window.showErrorMessage(text, "Go to error");
    if (choice === "Go to error") {
      const pos = new vscode.Position(line - 1, 0);
      editor.selection = new vscode.Selection(pos, pos);
      editor.revealRange(new vscode.Range(pos, pos));
    }
  } else {
    void vscode.window.showErrorMessage(text);
  }
}

function reportError(err: unknown): void {
  if (err instanceof BinaryNotFoundError) {
    void vscode.window
      .showErrorMessage(err.message, "Open Settings")
      .then((choice) => {
        if (choice === "Open Settings") {
          void vscode.commands.executeCommand(
            "workbench.action.openSettings",
            "fmBridge.binaryPath",
          );
        }
      });
    return;
  }
  const message = err instanceof Error ? err.message : String(err);
  void vscode.window.showErrorMessage(`fm-bridge: ${message}`);
}

// ─── Diagnostics ───

function registerDiagnostics(
  context: vscode.ExtensionContext,
  collection: vscode.DiagnosticCollection,
): void {
  const timers = new Map<string, NodeJS.Timeout>();

  const validate = async (doc: vscode.TextDocument): Promise<void> => {
    if (doc.languageId !== LANGUAGE) {
      return;
    }
    try {
      const resp = await parseScript(doc.getText());
      if (resp.status === "ok") {
        collection.delete(doc.uri);
        return;
      }
      // Prefer the full errors[] list (one squiggle per problem); fall back to
      // the single error/error_line for older binaries.
      const items =
        resp.errors && resp.errors.length > 0
          ? resp.errors
          : [{ line: resp.error_line ?? 0, message: resp.error ?? "Invalid .fmscript" }];
      collection.set(
        doc.uri,
        items.map((e) => toDiagnostic(doc, e.message, e.line)),
      );
    } catch {
      // Binary missing / unreachable: don't spam diagnostics. The explicit
      // read/write commands surface that error with actionable guidance.
      collection.delete(doc.uri);
    }
  };

  const scheduleValidate = (doc: vscode.TextDocument): void => {
    if (doc.languageId !== LANGUAGE) {
      return;
    }
    const validateOnType = vscode.workspace
      .getConfiguration("fmBridge")
      .get<boolean>("validateOnType", true);
    if (!validateOnType) {
      return;
    }
    const key = doc.uri.toString();
    const existing = timers.get(key);
    if (existing) {
      clearTimeout(existing);
    }
    timers.set(
      key,
      setTimeout(() => {
        timers.delete(key);
        void validate(doc);
      }, 400),
    );
  };

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((doc) => void validate(doc)),
    vscode.workspace.onDidSaveTextDocument((doc) => void validate(doc)),
    vscode.workspace.onDidChangeTextDocument((e) => scheduleValidate(e.document)),
    vscode.workspace.onDidCloseTextDocument((doc) => collection.delete(doc.uri)),
  );

  // Validate already-open .fmscript documents on activation.
  for (const doc of vscode.workspace.textDocuments) {
    void validate(doc);
  }
}

function toDiagnostic(
  doc: vscode.TextDocument,
  message: string | undefined,
  line: number | undefined,
): vscode.Diagnostic {
  const lineIndex =
    line && line > 0 ? Math.min(line - 1, doc.lineCount - 1) : 0;
  const textLine = doc.lineAt(lineIndex);
  // Squiggle from the first non-blank char to end of line (skip indentation).
  const start = new vscode.Position(
    lineIndex,
    textLine.isEmptyOrWhitespace ? 0 : textLine.firstNonWhitespaceCharacterIndex,
  );
  const range = new vscode.Range(start, textLine.range.end);
  const diag = new vscode.Diagnostic(
    range,
    message ?? "Invalid .fmscript",
    vscode.DiagnosticSeverity.Error,
  );
  diag.source = "fm-bridge";
  return diag;
}
