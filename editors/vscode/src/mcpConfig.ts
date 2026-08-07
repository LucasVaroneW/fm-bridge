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
import { ensureStableBinaries } from "./stableBin";

interface ClientTarget {
  /** Label shown in the picker. */
  label: string;
  /** Config file syntax. Codex is the odd one out: TOML, not JSON. */
  format?: "json" | "toml";
  /** Top-level key the server lives under in this client's config. */
  rootKey: "mcp" | "mcpServers" | "servers" | "mcp_servers";
  /** The per-server value (shape differs per client). */
  serverValue: (bin: string, env: Record<string, string>) => unknown;
  /**
   * Candidate config paths, most likely first. The first that already exists
   * wins; otherwise the first candidate is the default for a new file.
   *
   * A list rather than one path because several clients have moved theirs
   * between releases, and writing to a location nobody reads is worse than
   * saying we couldn't find it.
   */
  configCandidates: () => string[];
  /** Seeded into a brand-new file (e.g. OpenCode's $schema). */
  newFileExtras?: Record<string, unknown>;
}

/** Where this client's config lives, or would live. */
function configPathOf(client: ClientTarget): string | undefined {
  const candidates = client.configCandidates().filter((c) => c.length > 0);
  return candidates.find((c) => fs.existsSync(c)) ?? candidates[0];
}

/** Has this client been set up on this machine at all? */
function isDetected(client: ClientTarget): boolean {
  return client.configCandidates().some((c) => fs.existsSync(c));
}

/**
 * Environment handed to the MCP server.
 *
 * `FMBRIDGE_CONFIG` is what makes live data work over MCP at all: the AI client
 * starts the server from its own directory — never the user's project — so
 * without this the server searches the wrong tree and reports that no
 * connection is configured.
 */
function serverEnv(): Record<string, string> {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (!folder) {
    return {};
  }
  const config = path.join(folder, ".fm-bridge.toml");
  return fs.existsSync(config) ? { FMBRIDGE_CONFIG: config } : {};
}

/** Drop the key entirely when empty, so configs stay clean. */
function withEnv(
  base: Record<string, unknown>,
  key: string,
  env: Record<string, string>,
): Record<string, unknown> {
  return Object.keys(env).length > 0 ? { ...base, [key]: env } : base;
}

const home = (...p: string[]) => path.join(os.homedir(), ...p);

/** OpenCode: prefer an existing opencode.json[c], else default to .jsonc. */
function opencodePaths(): string[] {
  const dir = home(".config", "opencode");
  return [path.join(dir, "opencode.jsonc"), path.join(dir, "opencode.json")];
}

/** Claude Desktop's config path is OS-specific. */
function claudeDesktopPaths(): string[] {
  if (process.platform === "win32") {
    const appData = process.env.APPDATA ?? home("AppData", "Roaming");
    return [path.join(appData, "Claude", "claude_desktop_config.json")];
  }
  if (process.platform === "darwin") {
    return [
      home(
        "Library",
        "Application Support",
        "Claude",
        "claude_desktop_config.json",
      ),
    ];
  }
  return [home(".config", "Claude", "claude_desktop_config.json")];
}

/**
 * Antigravity is a VS Code fork from the Windsurf lineage, and that family has
 * moved its MCP config around. Try the known shapes and let detection decide,
 * rather than guessing one and writing a file the IDE never reads.
 */
function antigravityPaths(): string[] {
  const base = [
    home(".antigravity", "mcp_config.json"),
    home(".codeium", "windsurf", "mcp_config.json"),
  ];
  if (process.platform === "darwin") {
    base.push(
      home("Library", "Application Support", "Antigravity", "mcp_config.json"),
    );
  }
  if (process.platform === "win32") {
    const appData = process.env.APPDATA ?? home("AppData", "Roaming");
    base.push(path.join(appData, "Antigravity", "mcp_config.json"));
  }
  return base;
}

const CLIENTS: ClientTarget[] = [
  {
    label: "Claude Code",
    rootKey: "mcpServers",
    serverValue: (bin, env) =>
      withEnv({ type: "stdio", command: bin, args: ["mcp"] }, "env", env),
    configCandidates: () => [home(".claude.json")],
  },
  {
    label: "Claude Desktop",
    rootKey: "mcpServers",
    serverValue: (bin, env) =>
      withEnv({ command: bin, args: ["mcp"] }, "env", env),
    configCandidates: claudeDesktopPaths,
  },
  {
    label: "Cursor",
    rootKey: "mcpServers",
    serverValue: (bin, env) =>
      withEnv({ command: bin, args: ["mcp"] }, "env", env),
    configCandidates: () => [home(".cursor", "mcp.json")],
  },
  {
    label: "Antigravity",
    rootKey: "mcpServers",
    serverValue: (bin, env) =>
      withEnv({ command: bin, args: ["mcp"] }, "env", env),
    configCandidates: antigravityPaths,
  },
  {
    label: "Codex",
    // Codex configures MCP in TOML, not JSON.
    format: "toml",
    rootKey: "mcp_servers",
    serverValue: (bin, env) =>
      withEnv({ command: bin, args: ["mcp"] }, "env", env),
    configCandidates: () => [home(".codex", "config.toml")],
  },
  {
    label: "OpenCode",
    rootKey: "mcp",
    serverValue: (bin, env) =>
      // OpenCode names the field `environment`, not `env`.
      withEnv(
        { type: "local", command: [bin, "mcp"], enabled: true },
        "environment",
        env,
      ),
    configCandidates: opencodePaths,
    newFileExtras: { $schema: "https://opencode.ai/config.json" },
  },
  {
    label: "VS Code (this workspace)",
    rootKey: "servers",
    serverValue: (bin, env) =>
      withEnv({ type: "stdio", command: bin, args: ["mcp"] }, "env", env),
    configCandidates: () => {
      const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      return folder ? [path.join(folder, ".vscode", "mcp.json")] : [];
    },
  },
];

// ── TOML (Codex) ────────────────────────────────────────────────────────────

function tomlString(s: string): string {
  return `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** Render one server as a `[mcp_servers.fm-bridge]` block. */
function tomlBlock(rootKey: string, value: Record<string, unknown>): string {
  const lines = [`[${rootKey}.fm-bridge]`];
  for (const [k, v] of Object.entries(value)) {
    if (typeof v === "string") {
      lines.push(`${k} = ${tomlString(v)}`);
    } else if (Array.isArray(v)) {
      lines.push(`${k} = [${v.map((x) => tomlString(String(x))).join(", ")}]`);
    } else if (v && typeof v === "object") {
      // Inline table: `env = { KEY = "value" }`.
      const inner = Object.entries(v as Record<string, string>)
        .map(([ik, iv]) => `${ik} = ${tomlString(iv)}`)
        .join(", ");
      lines.push(`${k} = { ${inner} }`);
    }
  }
  return lines.join("\n") + "\n";
}

/**
 * Replace an existing `[mcp_servers.fm-bridge]` block, or append one.
 *
 * Deliberately textual rather than a parse/serialize round-trip: a Codex config
 * is hand-written and full of comments and ordering the user cares about, and
 * rewriting the whole file to change one block would throw all of that away.
 */
function mergeToml(existing: string, rootKey: string, block: string): string {
  // `[ \t]*`, not `\s*`: `\s` matches newlines, so it would swallow the blank
  // line separating this block from the previous one — every re-apply would
  // eat one more, slowly mangling the file.
  const header = new RegExp(
    `^[ \\t]*\\[${escapeRe(rootKey)}\\.(?:fm-bridge|"fm-bridge")\\][ \\t]*$`,
    "m",
  );
  const match = header.exec(existing);
  if (!match) {
    const sep = existing.length === 0 || existing.endsWith("\n") ? "" : "\n";
    return `${existing}${sep}\n${block}`;
  }
  // The block runs until the next table header or end of file.
  const start = match.index;
  const after = existing.slice(start + match[0].length);
  const nextHeader = /^\s*\[/m.exec(after);
  const end =
    nextHeader === undefined || nextHeader === null
      ? existing.length
      : start + match[0].length + nextHeader.index;
  return existing.slice(0, start) + block + existing.slice(end);
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** The standalone snippet (for the copy-to-clipboard path). */
function snippetFor(client: ClientTarget, bin: string): string {
  const value = client.serverValue(bin, serverEnv()) as Record<string, unknown>;
  if (client.format === "toml") {
    return tomlBlock(client.rootKey, value);
  }
  return JSON.stringify({ [client.rootKey]: { "fm-bridge": value } }, null, 2);
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
async function applyToConfig(
  client: ClientTarget,
  bin: string,
): Promise<ApplyResult> {
  const file = configPathOf(client);
  if (!file) {
    throw new Error(
      `Don't know where ${client.label} stores its config on this OS.`,
    );
  }
  await fs.promises.mkdir(path.dirname(file), { recursive: true });

  // TOML clients get a textual merge that preserves comments and ordering.
  if (client.format === "toml") {
    const existed = fs.existsSync(file);
    let text = existed ? await fs.promises.readFile(file, "utf8") : "";
    if (existed) {
      await fs.promises.copyFile(file, `${file}.bak`);
    }
    const value = client.serverValue(bin, serverEnv()) as Record<
      string,
      unknown
    >;
    text = mergeToml(text, client.rootKey, tomlBlock(client.rootKey, value));
    await fs.promises.writeFile(file, text, "utf8");
    return { file, created: !existed, backedUp: existed };
  }

  let root: Record<string, unknown> = {};
  let created = true;
  let backedUp = false;

  if (fs.existsSync(file)) {
    created = false;
    const raw = await fs.promises.readFile(file, "utf8");
    if (raw.trim().length > 0) {
      const parsed = parseJsonc(raw); // throws on garbage → caller handles
      if (
        typeof parsed !== "object" ||
        parsed === null ||
        Array.isArray(parsed)
      ) {
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
    typeof existing === "object" &&
    existing !== null &&
    !Array.isArray(existing)
      ? (existing as Record<string, unknown>)
      : {};
  servers["fm-bridge"] = client.serverValue(bin, serverEnv());
  root[client.rootKey] = servers;

  await fs.promises.writeFile(
    file,
    `${JSON.stringify(root, null, 2)}\n`,
    "utf8",
  );
  return { file, created, backedUp };
}

/**
 * Resolve the binary, pick a client, then offer to apply the MCP config
 * directly (recommended) or copy it. Messages name the exact file and the
 * restart step so a human — or an AI reading them — knows precisely what happened.
 */
export async function copyMcpConfigCommand(): Promise<void> {
  const resolved = resolveBinaryPath();
  if (!resolved) {
    void vscode.window.showErrorMessage(new BinaryNotFoundError().message);
    return;
  }
  // Point clients at the version-independent copy, so this config survives the
  // next extension update instead of dangling at a deleted folder.
  const bin = ensureStableBinaries(resolved) ?? resolved;

  // Show which clients are actually present, so nobody writes a config into a
  // location their IDE never reads.
  const items = CLIENTS.map((c) => {
    const detected = isDetected(c);
    return {
      label: `${detected ? "$(check)" : "$(circle-outline)"} ${c.label}`,
      description: detected ? "detected" : "not detected here",
      detail: configPathOf(c) ?? "(unknown path on this OS)",
      client: c,
      detected,
    };
  }).sort((a, b) => Number(b.detected) - Number(a.detected));

  const pick = await vscode.window.showQuickPick(items, {
    title: "fm-bridge: set up MCP for which AI client?",
    placeHolder: "Pick the AI agent you want to give the fm-bridge tools to",
  });
  if (!pick) {
    return;
  }
  const client = pick.client;

  const APPLY = `Apply to ${client.label}'s config`;
  const COPY = "Copy to clipboard instead";
  // When the client was not detected, the path is a best guess — offer the
  // clipboard first so the user places it where their install really looks.
  const how = await vscode.window.showQuickPick(
    pick.detected ? [APPLY, COPY] : [COPY, APPLY],
    {
      title: `fm-bridge: ${client.label}`,
      placeHolder: pick.detected
        ? pick.detail
        : `${client.label} was not detected — ${pick.detail} is a best guess`,
    },
  );
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
