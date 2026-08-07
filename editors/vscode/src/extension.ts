// fm-bridge VS Code extension entry point.
//
// Wires up the human-facing features over the fm-bridge Rust binary:
//   - Read script from clipboard  → opens the decoded .fmscript
//   - Write script to clipboard   → encodes the active .fmscript for FileMaker
//   - Diagnostics                 → underlines format errors (on type + on save)
//   - Autocomplete                → step names from the binary's catalog
//   - Inspect / slice             → navigate a FMSaveAsXML export
//   - Live data (connect/query/doctor) → rows from a hosted file over ODBC
//
// This is one of two doors onto the same engine; the other is `fm-bridge mcp`.
// Neither requires the other — the extension works with no AI configured.
//
// All FileMaker know-how lives in the binary; this file is glue.

import * as vscode from "vscode";
import {
  BinaryNotFoundError,
  parseScript,
  readClipboard,
  reformat,
  resetBinaryCache,
  resolveBinaryPath,
  resolveIds,
  writeClipboard,
} from "./bridge";
import { StepCompletionProvider, resetCatalogCache } from "./completion";
import {
  connectCommand,
  doctorCommand,
  queryCommand,
  setDataLogChannel,
} from "./data";
import { inspectXmlCommand, sliceCommand } from "./inspect";
import { copyMcpConfigCommand } from "./mcpConfig";
import { StepFixProvider } from "./quickfix";
import { ensureStableBinaries } from "./stableBin";

const LANGUAGE = "fmscript";

/** Diagnostic log, visible in Output → "fm-bridge". Set in activate(). */
let output: vscode.OutputChannel | undefined;
function log(message: string): void {
  output?.appendLine(`[${new Date().toLocaleTimeString()}] ${message}`);
}

export function activate(context: vscode.ExtensionContext): void {
  output = vscode.window.createOutputChannel("fm-bridge");
  context.subscriptions.push(output);
  setDataLogChannel(output);
  const binary = resolveBinaryPath();
  log(`activated · binary: ${binary ?? "NOT FOUND"}`);

  // Refresh the version-independent copy of the binaries on every activation,
  // so MCP clients configured against a previous release keep working after an
  // update instead of pointing at a folder VS Code has deleted.
  if (binary) {
    const stable = ensureStableBinaries(binary);
    log(stable ? `stable copy: ${stable}` : "stable copy: unavailable");
  }

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
    vscode.commands.registerCommand("fm-bridge.inspectXml", inspectXmlCommand),
    vscode.commands.registerCommand("fm-bridge.slice", sliceCommand),
    vscode.commands.registerCommand(
      "fm-bridge.copyMcpConfig",
      copyMcpConfigCommand,
    ),
    vscode.commands.registerCommand("fm-bridge.formatInline", () =>
      reformatActive("inline"),
    ),
    vscode.commands.registerCommand("fm-bridge.formatIndented", () =>
      reformatActive("indented"),
    ),
    vscode.commands.registerCommand("fm-bridge.showLog", () => output?.show()),
    // Live data (ODBC). The human door: no terminal, no TOML, no AI needed.
    vscode.commands.registerCommand("fm-bridge.dataConnect", connectCommand),
    vscode.commands.registerCommand("fm-bridge.dataDoctor", doctorCommand),
    vscode.commands.registerCommand("fm-bridge.dataQuery", queryCommand),
    vscode.commands.registerCommand(
      "fm-bridge.resolveLayoutIds",
      resolveLayoutIds,
    ),
    vscode.languages.registerCompletionItemProvider(
      LANGUAGE,
      new StepCompletionProvider(),
    ),
    vscode.languages.registerCodeActionsProvider(
      LANGUAGE,
      new StepFixProvider(),
      {
        providedCodeActionKinds: StepFixProvider.kinds,
      },
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

async function resolveLayoutIds(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    void vscode.window.showErrorMessage(
      "fm-bridge: open a .fmscript file first.",
    );
    return;
  }
  try {
    const files = await vscode.window.showOpenDialog({
      filters: { FMSaveAsXML: ["xml"] },
      title: "Choose a FMSaveAsXML export to resolve layout IDs",
    });
    if (!files || files.length === 0) {
      return;
    }
    const resp = await resolveIds(editor.document.getText(), files[0].fsPath);
    if (resp.status !== "ok" || resp.script_text === undefined) {
      void vscode.window.showErrorMessage(
        `fm-bridge: ${resp.error ?? "could not resolve layout IDs"}`,
      );
      return;
    }
    const fullRange = new vscode.Range(
      editor.document.positionAt(0),
      editor.document.positionAt(editor.document.getText().length),
    );
    await editor.edit((eb) => eb.replace(fullRange, resp.script_text!));
    void vscode.window.showInformationMessage(
      "fm-bridge: layout IDs resolved.",
    );
  } catch (err) {
    reportError(err);
  }
}

/**
 * Re-render the active .fmscript in the given style (inline = one line per step
 * so line numbers match FileMaker; indented = readable multi-line) and replace
 * the document in place. Side-effect free on the clipboard.
 */
async function reformatActive(style: "inline" | "indented"): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== LANGUAGE) {
    void vscode.window.showErrorMessage(
      "fm-bridge: open a .fmscript file first.",
    );
    return;
  }
  try {
    const resp = await reformat(editor.document.getText(), style);
    if (resp.status !== "ok" || resp.script_text === undefined) {
      await showWriteError(editor, resp.error, resp.error_line);
      return;
    }
    const full = new vscode.Range(
      editor.document.positionAt(0),
      editor.document.positionAt(editor.document.getText().length),
    );
    await editor.edit((e) => e.replace(full, resp.script_text as string));
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
    const name = doc.uri.path.split("/").pop() ?? doc.uri.path;
    try {
      const resp = await parseScript(doc.getText());
      // Show warnings even when status is ok (e.g. missing layout IDs).
      if (resp.errors && resp.errors.length > 0) {
        collection.set(
          doc.uri,
          resp.errors.map((e) =>
            toDiagnostic(doc, e.message, e.line, e.severity),
          ),
        );
        log(`validated ${name}: ${resp.errors.length} warning(s)`);
        return;
      }
      if (resp.status === "ok") {
        collection.delete(doc.uri);
        log(`validated ${name}: ok`);
        return;
      }
      // Prefer the full errors[] list (one squiggle per problem); fall back to
      // the single error/error_line for older binaries.
      const items =
        resp.errors && resp.errors.length > 0
          ? resp.errors
          : [
              {
                line: resp.error_line ?? 0,
                message: resp.error ?? "Invalid .fmscript",
              },
            ];
      collection.set(
        doc.uri,
        items.map((e) => toDiagnostic(doc, e.message, e.line, e.severity)),
      );
      log(`validated ${name}: ${items.length} error(s)`);
    } catch (err) {
      // Binary missing / unreachable: don't spam diagnostics. The explicit
      // read/write commands surface that error with actionable guidance.
      collection.delete(doc.uri);
      const message = err instanceof Error ? err.message : String(err);
      log(`validate ${name} failed: ${message}`);
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
    vscode.workspace.onDidChangeTextDocument((e) =>
      scheduleValidate(e.document),
    ),
    vscode.workspace.onDidCloseTextDocument((doc) =>
      collection.delete(doc.uri),
    ),
  );

  // Validate already-open .fmscript documents on activation.
  const open = vscode.workspace.textDocuments;
  const fmDocs = open.filter((d) => d.languageId === LANGUAGE);
  log(
    `open documents: ${open.length}, of which fmscript: ${fmDocs.length}` +
      (fmDocs.length === 0 && open.length > 0
        ? " — if your .fmscript shows nothing, check the language mode (bottom-right) says 'FileMaker Script'"
        : ""),
  );
  for (const doc of fmDocs) {
    void validate(doc);
  }
}

function toDiagnostic(
  doc: vscode.TextDocument,
  message: string | undefined,
  line: number | undefined,
  severity?: string,
): vscode.Diagnostic {
  const lineIndex =
    line && line > 0 ? Math.min(line - 1, doc.lineCount - 1) : 0;
  const textLine = doc.lineAt(lineIndex);
  // Squiggle from the first non-blank char to end of line (skip indentation).
  const start = new vscode.Position(
    lineIndex,
    textLine.isEmptyOrWhitespace
      ? 0
      : textLine.firstNonWhitespaceCharacterIndex,
  );
  const range = new vscode.Range(start, textLine.range.end);
  const sev =
    severity === "warning"
      ? vscode.DiagnosticSeverity.Warning
      : vscode.DiagnosticSeverity.Error;
  const diag = new vscode.Diagnostic(
    range,
    message ?? "Invalid .fmscript",
    sev,
  );
  diag.source = "fm-bridge";
  return diag;
}
