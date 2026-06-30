// "Copy MCP config" command.
//
// The .vsix ships the fm-bridge binary but cannot edit an external AI client's
// config (OpenCode, Claude Desktop, Cursor…). This command bridges that gap with
// zero typing: it resolves the bundled binary's real path and hands the user a
// ready-to-paste MCP config block for the client they pick — no fragile manual
// paths, no Rust install.

import * as vscode from "vscode";
import { BinaryNotFoundError, resolveBinaryPath } from "./bridge";

interface ClientTarget {
  /** Label shown in the picker. */
  label: string;
  /** Where the snippet has to be pasted. */
  detail: string;
  /** Build the JSON config block for this client, given the binary path. */
  build: (bin: string) => unknown;
}

const CLIENTS: ClientTarget[] = [
  {
    label: "OpenCode",
    detail: "~/.config/opencode/opencode.jsonc  (key: mcp)",
    build: (bin) => ({
      mcp: {
        "fm-bridge": { type: "local", command: [bin, "mcp"], enabled: true },
      },
    }),
  },
  {
    label: "Claude Desktop",
    detail: "%APPDATA%\\Claude\\claude_desktop_config.json  (key: mcpServers)",
    build: (bin) => ({
      mcpServers: { "fm-bridge": { command: bin, args: ["mcp"] } },
    }),
  },
  {
    label: "Cursor",
    detail: "~/.cursor/mcp.json  (key: mcpServers)",
    build: (bin) => ({
      mcpServers: { "fm-bridge": { command: bin, args: ["mcp"] } },
    }),
  },
];

/**
 * Resolve the binary, ask which AI client to target, then copy a paste-ready
 * MCP config block to the clipboard. Surfaces the destination file and the next
 * step (restart the client) so a human — or an AI reading the message — knows
 * exactly what to do.
 */
export async function copyMcpConfigCommand(): Promise<void> {
  const bin = resolveBinaryPath();
  if (!bin) {
    void vscode.window.showErrorMessage(new BinaryNotFoundError().message);
    return;
  }

  const pick = await vscode.window.showQuickPick(
    CLIENTS.map((c) => ({ label: c.label, detail: c.detail, client: c })),
    {
      title: "fm-bridge: MCP config for which AI client?",
      placeHolder: "Pick the AI agent you want to give the fm-bridge tools to",
    },
  );
  if (!pick) {
    return; // user dismissed
  }

  // JSON.stringify escapes the Windows backslashes in the path for us.
  const snippet = JSON.stringify(pick.client.build(bin), null, 2);
  await vscode.env.clipboard.writeText(snippet);

  const choice = await vscode.window.showInformationMessage(
    `fm-bridge: MCP config for ${pick.label} copied to the clipboard. ` +
      `Paste it into ${pick.client.detail}, then restart ${pick.label}. ` +
      `If the file already has that key, merge instead of overwriting.`,
    "Show snippet",
  );
  if (choice === "Show snippet") {
    const doc = await vscode.workspace.openTextDocument({
      language: "json",
      content: snippet,
    });
    await vscode.window.showTextDocument(doc);
  }
}
