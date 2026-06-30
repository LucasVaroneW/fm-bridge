// "Set up MCP for an AI agent" command.
//
// The .vsix ships the fm-bridge binary but cannot, on its own, register the MCP
// server in an external AI client (OpenCode, Claude Desktop, Cursor…). This
// command closes that gap with zero typing: it resolves the bundled binary's
// real path and either
//   - writes/merges the MCP entry straight into the client's config file
//     (detecting the per-OS path, backing the file up first), or
//   - copies a ready-to-paste block to the clipboard (universal fallback).

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";
import { BinaryNotFoundError, resolveBinaryPath } from "./bridge";

interface ClientTarget {
  /** Label shown in the picker. */
  label: string;
  /** Top-level key the server lives under in this client's config. */
  rootKey: "mcp" | "mcpServers";
  /** The per-server value (shape differs per client). */
  serverValue: (bin: string) => unknown;
  /** Resolve the config file path for this OS; undefined if unknown. */
  configPath: () => string | undefined;
  /** Seeded into a brand-new file (e.g. OpenCode's $schema). */
  newFileExtras?: Record<string, unknown>;
}

/** OpenCode: prefer an existing opencode.json[c], else default to .jsonc. */
function opencodePath(): string {
  const dir = path.join(os.homedir(), ".config", "opencode");
  for (const name of ["opencode.json", "opencode.jsonc"]) {
    const p = path.join(dir, name);
    if (fs.existsSync(p)) {
      return p;
    }
  }
  return path.join(dir, "opencode.jsonc");
}

/** Claude Desktop's config path is OS-specific. */
function claudeDesktopPath(): string {
  if (process.platform === "win32") {
    const appData = process.env.APPDATA ?? path.join(os.homedir(), "AppData", "Roaming");
    return path.join(appData, "Claude", "claude_desktop_config.json");
  }
  if (process.platform === "darwin") {
    return path.join(
      os.homedir(),
      "Library",
      "Application Support",
      "Claude",
      "claude_desktop_config.json",
    );
  }
  return path.join(os.homedir(), ".config", "Claude", "claude_desktop_config.json");
}

const CLIENTS: ClientTarget[] = [
  {
    label: "OpenCode",
    rootKey: "mcp",
    serverValue: (bin) => ({ type: "local", command: [bin, "mcp"], enabled: true }),
    configPath: opencodePath,
    newFileExtras: { $schema: "https://opencode.ai/config.json" },
  },
  {
    label: "Claude Desktop",
    rootKey: "mcpServers",
    serverValue: (bin) => ({ command: bin, args: ["mcp"] }),
    configPath: claudeDesktopPath,
  },
  {
    label: "Cursor",
    rootKey: "mcpServers",
    serverValue: (bin) => ({ command: bin, args: ["mcp"] }),
    configPath: () => path.join(os.homedir(), ".cursor", "mcp.json"),
  },
];

/** The standalone snippet (for the copy-to-clipboard path). */
function snippetFor(client: ClientTarget, bin: string): string {
  return JSON.stringify(
    { [client.rootKey]: { "fm-bridge": client.serverValue(bin) } },
    null,
    2,
  );
}

/**
 * Strip `//` and `/* *\/` comments from JSONC, respecting string literals, then
 * drop trailing commas — enough to JSON.parse a hand-edited config (OpenCode
 * uses .jsonc). Best-effort: if it still doesn't parse, the caller falls back to
 * copy-to-clipboard rather than risk clobbering the file.
 */
function parseJsonc(text: string): unknown {
  let out = "";
  let inStr = false;
  let line = false;
  let block = false;
  let esc = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    const n = text[i + 1];
    if (line) {
      if (c === "\n") {
        line = false;
        out += c;
      }
      continue;
    }
    if (block) {
      if (c === "*" && n === "/") {
        block = false;
        i++;
      }
      continue;
    }
    if (inStr) {
      out += c;
      if (esc) {
        esc = false;
      } else if (c === "\\") {
        esc = true;
      } else if (c === '"') {
        inStr = false;
      }
      continue;
    }
    if (c === '"') {
      inStr = true;
      out += c;
      continue;
    }
    if (c === "/" && n === "/") {
      line = true;
      i++;
      continue;
    }
    if (c === "/" && n === "*") {
      block = true;
      i++;
      continue;
    }
    out += c;
  }
  const noTrailingCommas = out.replace(/,(\s*[}\]])/g, "$1");
  return JSON.parse(noTrailingCommas);
}

interface ApplyResult {
  file: string;
  created: boolean;
  backedUp: boolean;
}

/** Write/merge the fm-bridge server into the client's config file. */
async function applyToConfig(client: ClientTarget, bin: string): Promise<ApplyResult> {
  const file = client.configPath();
  if (!file) {
    throw new Error(`Don't know where ${client.label} stores its config on this OS.`);
  }
  await fs.promises.mkdir(path.dirname(file), { recursive: true });

  let root: Record<string, unknown> = {};
  let created = true;
  let backedUp = false;

  if (fs.existsSync(file)) {
    created = false;
    const raw = await fs.promises.readFile(file, "utf8");
    if (raw.trim().length > 0) {
      const parsed = parseJsonc(raw); // throws on garbage → caller handles
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
        throw new Error(`${path.basename(file)} is not a JSON object.`);
      }
      root = parsed as Record<string, unknown>;
    }
    // Keep a one-shot backup before rewriting (comments/formatting normalize).
    await fs.promises.copyFile(file, `${file}.bak`);
    backedUp = true;
  } else if (client.newFileExtras) {
    root = { ...client.newFileExtras };
  }

  const existing = root[client.rootKey];
  const servers =
    typeof existing === "object" && existing !== null && !Array.isArray(existing)
      ? (existing as Record<string, unknown>)
      : {};
  servers["fm-bridge"] = client.serverValue(bin);
  root[client.rootKey] = servers;

  await fs.promises.writeFile(file, `${JSON.stringify(root, null, 2)}\n`, "utf8");
  return { file, created, backedUp };
}

/**
 * Resolve the binary, pick a client, then offer to apply the MCP config
 * directly (recommended) or copy it. Messages name the exact file and the
 * restart step so a human — or an AI reading them — knows precisely what happened.
 */
export async function copyMcpConfigCommand(): Promise<void> {
  const bin = resolveBinaryPath();
  if (!bin) {
    void vscode.window.showErrorMessage(new BinaryNotFoundError().message);
    return;
  }

  const pick = await vscode.window.showQuickPick(
    CLIENTS.map((c) => ({
      label: c.label,
      detail: c.configPath() ?? "(unknown path on this OS)",
      client: c,
    })),
    {
      title: "fm-bridge: set up MCP for which AI client?",
      placeHolder: "Pick the AI agent you want to give the fm-bridge tools to",
    },
  );
  if (!pick) {
    return;
  }
  const client = pick.client;

  const APPLY = `Apply to ${client.label}'s config`;
  const COPY = "Copy to clipboard instead";
  const how = await vscode.window.showQuickPick([APPLY, COPY], {
    title: `fm-bridge: ${client.label}`,
    placeHolder: pick.detail,
  });
  if (!how) {
    return;
  }

  if (how === COPY) {
    await vscode.env.clipboard.writeText(snippetFor(client, bin));
    void vscode.window.showInformationMessage(
      `fm-bridge: config for ${client.label} copied. Paste it into ${pick.detail}, ` +
        `merge if the key exists, then restart ${client.label}.`,
    );
    return;
  }

  try {
    const res = await applyToConfig(client, bin);
    const note = res.created
      ? "created"
      : res.backedUp
        ? "updated (backup saved as .bak)"
        : "updated";
    const choice = await vscode.window.showInformationMessage(
      `fm-bridge: MCP set up in ${client.label} — ${path.basename(res.file)} ${note}. ` +
        `Restart ${client.label} to load the tools.`,
      "Open config",
    );
    if (choice === "Open config") {
      const doc = await vscode.workspace.openTextDocument(res.file);
      await vscode.window.showTextDocument(doc);
    }
  } catch (err) {
    // Don't risk a half-written file: fall back to copy + open the target.
    const message = err instanceof Error ? err.message : String(err);
    await vscode.env.clipboard.writeText(snippetFor(client, bin));
    void vscode.window.showWarningMessage(
      `fm-bridge: couldn't edit ${client.label}'s config automatically (${message}). ` +
        `The config block was copied to your clipboard instead — paste it into ${pick.detail}.`,
    );
  }
}
