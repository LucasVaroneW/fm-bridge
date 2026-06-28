// Step-name autocomplete sourced from `fm-bridge steps`.
//
// The catalog (names + shape + block behavior) comes entirely from the binary,
// so the extension can never offer a step the installed binary doesn't support.
// Each completion inserts a snippet template chosen by the step's shape, with a
// few name-specific overrides for the richer steps.

import * as vscode from "vscode";
import { StepInfo, steps } from "./bridge";

let catalogCache: StepInfo[] | undefined;

/** Load (and memoize) the step catalog. Failures degrade to no completions. */
async function getCatalog(): Promise<StepInfo[]> {
  if (catalogCache) {
    return catalogCache;
  }
  catalogCache = await steps();
  return catalogCache;
}

/** Drop the cached catalog (e.g. after the binary path changes). */
export function resetCatalogCache(): void {
  catalogCache = undefined;
}

/**
 * Build the snippet body for a step. Name-specific templates win; otherwise we
 * fall back to a generic template per shape. Block openers (If/Loop) expand with
 * their matching closer so the user gets a complete, valid block.
 */
function templateFor(step: StepInfo): vscode.SnippetString {
  // Name-specific, high-value templates.
  switch (step.en) {
    case "Set Variable":
      return new vscode.SnippetString("Set Variable [${1:\\$var} = ${2:value}]");
    case "Set Field":
      return new vscode.SnippetString("Set Field [${1:Table::Field}; ${2:value}]");
    case "If":
      return new vscode.SnippetString("If [${1:condition}]\n\t$0\nEnd If");
    case "Else If":
      return new vscode.SnippetString("Else If [${1:condition}]");
    case "Loop":
      return new vscode.SnippetString("Loop\n\t$0\nEnd Loop");
    case "Show Custom Dialog":
      return new vscode.SnippetString(
        'Show Custom Dialog [Title: ${1:"Title"}; Message: ${2:"Message"}; Buttons: ${3:"OK"}]',
      );
    case "Perform Script":
    case "Perform Script on Server":
      return new vscode.SnippetString(
        `${step.en} [\${1:"ScriptName"} #\${2:id}; \${3:parameter}]`,
      );
    case "Go to Layout":
      return new vscode.SnippetString('Go to Layout [${1:"LayoutName"} #${2:id}]');
    case "Go to Object":
      return new vscode.SnippetString('Go to Object [${1:"objectName"}]');
    case "Select Window":
      return new vscode.SnippetString("Select Window [${1:Current}]");
    case "New Window":
      return new vscode.SnippetString(
        'New Window [Style: ${1:Document}; Layout: ${2:"LayoutName"}]',
      );
    case "Perform Find":
      return new vscode.SnippetString(
        "Perform Find [\n\tFind: ${1:Table::Field} => ${2:value}\n]",
      );
    case "Insert from URL":
      return new vscode.SnippetString(
        'Insert from URL [Target: ${1:\\$response}; URL: ${2:"https://..."}; Dialog: Off; VerifySSL]',
      );
    case "Execute FileMaker Data API":
      return new vscode.SnippetString(
        "Execute FileMaker Data API [${1:\\$result}; ${2:jsonCalc}]",
      );
  }

  // Generic fallback by shape.
  switch (step.shape) {
    case "plain":
      return new vscode.SnippetString(step.en);
    case "comment":
      return new vscode.SnippetString("# ${1:comment}");
    case "set_state":
      return new vscode.SnippetString(`${step.en} [\${1:On}]`);
    case "go_to_record":
      return new vscode.SnippetString(`${step.en} [\${1:First}]`);
    case "adjust_window":
      return new vscode.SnippetString(`${step.en} [\${1:ResizeToFit}]`);
    case "opaque":
      return new vscode.SnippetString(`${step.en} [\${1:<xml>}]`);
    default:
      // calculation, calculation_with_restore, field_and_calc, etc.
      return new vscode.SnippetString(`${step.en} [\${1:calculation}]`);
  }
}

export class StepCompletionProvider implements vscode.CompletionItemProvider {
  async provideCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position,
  ): Promise<vscode.CompletionItem[]> {
    // Only complete the step name at the start of a line — not inside a calc,
    // a comment, or a disabled step.
    const linePrefix = document
      .lineAt(position.line)
      .text.slice(0, position.character);
    if (/[[\]"#]/.test(linePrefix) || linePrefix.trimStart().startsWith("//")) {
      return [];
    }

    let catalog: StepInfo[];
    try {
      catalog = await getCatalog();
    } catch {
      return [];
    }

    // Step names contain spaces ("Set Variable"), which VS Code treats as word
    // boundaries. Replace the whole partial step name (from the first non-blank
    // char to the cursor) so "Set V" → the full template, not "Set Set V…".
    const lineText = document.lineAt(position.line).text;
    const nameStart = new vscode.Position(
      position.line,
      lineText.length - lineText.trimStart().length,
    );
    const replaceRange = new vscode.Range(nameStart, position);

    return catalog.map((step) => {
      const item = new vscode.CompletionItem(
        step.en,
        vscode.CompletionItemKind.Function,
      );
      item.insertText = templateFor(step);
      item.range = replaceRange;
      item.detail = step.es ? `${step.en}  ·  ${step.es}` : step.en;
      const notes: string[] = [`shape: \`${step.shape}\``];
      if (step.opens_block) {
        notes.push("opens a block");
      }
      if (!step.has_id) {
        notes.push("⚠️ no FileMaker ID recorded — cannot be written yet");
      }
      item.documentation = new vscode.MarkdownString(notes.join("  \n"));
      // Match on the English name even when the user types lowercase.
      item.filterText = step.en;
      return item;
    });
  }
}
