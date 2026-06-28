// Quick Fix for step-name typos. When the linter emits a "did you mean 'If'?"
// diagnostic, offer a lightbulb that rewrites the step name in place.

import * as vscode from "vscode";

const SUGGESTION = /did you mean '([^']+)'/;

export class StepFixProvider implements vscode.CodeActionProvider {
  static readonly kinds = [vscode.CodeActionKind.QuickFix];

  provideCodeActions(
    document: vscode.TextDocument,
    _range: vscode.Range | vscode.Selection,
    context: vscode.CodeActionContext,
  ): vscode.CodeAction[] {
    const actions: vscode.CodeAction[] = [];
    for (const diag of context.diagnostics) {
      if (diag.source !== "fm-bridge") {
        continue;
      }
      const match = SUGGESTION.exec(diag.message);
      if (!match) {
        continue;
      }
      const suggestion = match[1];
      const nameRange = stepNameRange(document, diag.range.start.line);
      const fix = new vscode.CodeAction(
        `Change to '${suggestion}'`,
        vscode.CodeActionKind.QuickFix,
      );
      fix.edit = new vscode.WorkspaceEdit();
      fix.edit.replace(document.uri, nameRange, suggestion);
      fix.diagnostics = [diag];
      fix.isPreferred = true;
      actions.push(fix);
    }
    return actions;
  }
}

/**
 * The range covering just the step-name token on a line: from the first
 * non-blank char (skipping a `// ` disable marker) up to the ` [` that opens the
 * parameters, or end of line for parameterless steps.
 */
function stepNameRange(
  document: vscode.TextDocument,
  lineIndex: number,
): vscode.Range {
  const line = document.lineAt(lineIndex);
  const text = line.text;
  let startCol = line.firstNonWhitespaceCharacterIndex;
  if (text.slice(startCol).startsWith("// ")) {
    startCol += 3;
  }
  const bracket = text.indexOf(" [", startCol);
  const endCol = bracket >= 0 ? bracket : text.trimEnd().length;
  return new vscode.Range(lineIndex, startCol, lineIndex, endCol);
}
