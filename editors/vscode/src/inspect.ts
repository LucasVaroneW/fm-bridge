// fm-bridge inspect / slice commands.
//
// These drive the binary's file-based subcommands (not JSON mode): `inspect`
// turns a FMSaveAsXML database export into a navigable folder, and `slice`
// pares that folder down to the context around a few layouts. Both can run for
// a while on large (100MB+) exports, so they run inside a cancellable progress
// notification.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { BinaryNotFoundError, runInspect, runSlice } from "./bridge";

/** One entry of the inspect output's `layouts.json` index. */
interface LayoutIndexEntry {
  id: number;
  name: string;
  is_folder?: boolean;
}

/** Pick an FMSaveAsXML export, inspect it into a sibling folder, reveal it. */
export async function inspectXmlCommand(): Promise<void> {
  try {
    const xml = await pickXmlFile();
    if (!xml) {
      return;
    }
    // Default output next to the XML: <name>-inspect/. Let the user confirm.
    const defaultOut = path.join(
      path.dirname(xml),
      `${path.basename(xml, path.extname(xml))}-inspect`,
    );
    const outputDir = await vscode.window.showInputBox({
      title: "fm-bridge: inspect output folder",
      prompt: "Where to write the inspection output",
      value: defaultOut,
      ignoreFocusOut: true,
    });
    if (!outputDir) {
      return;
    }

    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `fm-bridge: inspecting ${path.basename(xml)}…`,
        cancellable: false,
      },
      async () => {
        await runInspect(xml, outputDir);
      },
    );

    await revealResult(
      path.join(outputDir, "manifest.json"),
      outputDir,
      `Inspected ${path.basename(xml)} → ${outputDir}`,
    );
  } catch (err) {
    reportError(err);
  }
}

/** Pick an inspect output folder + layouts, build a slice, open its summary. */
export async function sliceCommand(): Promise<void> {
  try {
    const outputDir = await pickFolder("Select an fm-bridge inspect output folder");
    if (!outputDir) {
      return;
    }
    if (!fs.existsSync(path.join(outputDir, "layouts.json"))) {
      void vscode.window.showErrorMessage(
        "fm-bridge: that folder has no layouts.json — run 'Inspect FMSaveAsXML' on it first.",
      );
      return;
    }

    const layouts = await pickLayouts(outputDir);
    if (!layouts || layouts.length === 0) {
      return;
    }

    const defaultSlice = path.join(
      path.dirname(outputDir),
      `slice-${sanitize(layouts[0])}`,
    );
    const sliceDir = await vscode.window.showInputBox({
      title: "fm-bridge: slice output folder",
      prompt: `Where to write the slice for ${layouts.length} layout(s)`,
      value: defaultSlice,
      ignoreFocusOut: true,
    });
    if (!sliceDir) {
      return;
    }

    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `fm-bridge: slicing ${layouts.length} layout(s)…`,
        cancellable: false,
      },
      async () => {
        await runSlice(outputDir, sliceDir, layouts);
      },
    );

    // The slice's prose summary is the natural thing to open for the user/AI.
    await revealResult(
      path.join(sliceDir, "slice_summary.md"),
      sliceDir,
      `Sliced ${layouts.length} layout(s) → ${sliceDir}`,
    );
  } catch (err) {
    reportError(err);
  }
}

// ─── helpers ───

async function pickXmlFile(): Promise<string | undefined> {
  // Default to the active editor's file if it looks like an XML export.
  const active = vscode.window.activeTextEditor?.document.uri;
  const picked = await vscode.window.showOpenDialog({
    title: "Select a FMSaveAsXML export",
    canSelectMany: false,
    defaultUri: active,
    filters: { "FileMaker XML export": ["xml"], "All files": ["*"] },
    openLabel: "Inspect",
  });
  return picked?.[0]?.fsPath;
}

async function pickFolder(title: string): Promise<string | undefined> {
  const picked = await vscode.window.showOpenDialog({
    title,
    canSelectFiles: false,
    canSelectFolders: true,
    canSelectMany: false,
    openLabel: "Select",
  });
  return picked?.[0]?.fsPath;
}

async function pickLayouts(outputDir: string): Promise<string[] | undefined> {
  let entries: LayoutIndexEntry[];
  try {
    const raw = fs.readFileSync(path.join(outputDir, "layouts.json"), "utf8");
    entries = JSON.parse(raw) as LayoutIndexEntry[];
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    void vscode.window.showErrorMessage(`fm-bridge: cannot read layouts.json — ${message}`);
    return undefined;
  }

  const items: vscode.QuickPickItem[] = entries
    .filter((e) => !e.is_folder && e.name.length > 0)
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((e) => ({ label: e.name, description: `#${e.id}` }));

  if (items.length === 0) {
    void vscode.window.showErrorMessage("fm-bridge: no layouts found in that inspect output.");
    return undefined;
  }

  const chosen = await vscode.window.showQuickPick(items, {
    title: "fm-bridge: layouts to slice around",
    placeHolder: "Pick one or more layouts (the slice is their transitive closure)",
    canPickMany: true,
    matchOnDescription: true,
  });
  return chosen?.map((c) => c.label);
}

/** Open the primary result file if present, else reveal the folder. Offer both. */
async function revealResult(
  primaryFile: string,
  folder: string,
  successMessage: string,
): Promise<void> {
  const openFolder = "Reveal folder";
  const openFile = "Open summary";
  const actions = fs.existsSync(primaryFile) ? [openFile, openFolder] : [openFolder];
  const choice = await vscode.window.showInformationMessage(
    `fm-bridge: ${successMessage}`,
    ...actions,
  );
  if (choice === openFile) {
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(primaryFile));
    await vscode.window.showTextDocument(doc);
  } else if (choice === openFolder) {
    await vscode.commands.executeCommand("revealFileInOS", vscode.Uri.file(folder));
  }
}

function sanitize(name: string): string {
  return name.replace(/[^A-Za-z0-9._-]+/g, "_").slice(0, 40);
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
