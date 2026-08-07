// The human door to live data.
//
// Everything here exists so a FileMaker developer can use the ODBC path
// without a terminal, without editing TOML, and without an AI in the loop. The
// AI door (MCP) drives the exact same engine commands — neither is required by
// the other.
//
// Three commands:
//   Connect to a database…  — a wizard that writes .fm-bridge.toml and stores
//                             the password outside the project
//   Diagnose connection     — the doctor, rendered with its fixes
//   Run a query…            — a SELECT, with results in a readable table
//
// No SQL knowledge is assumed for the first two; the third is for when you do
// know what you want to ask.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { BinaryNotFoundError, runJson, spawnBinary } from "./bridge";

const CONFIG_FILE = ".fm-bridge.toml";

interface DoctorCheck {
  check: string;
  ok: boolean;
  detail: string;
}
interface DoctorResult {
  ok: boolean;
  checks: DoctorCheck[];
}
interface DatabaseEntry {
  name: string;
  server: string;
  odbc_name: string;
  xml: string | null;
}
interface DatabaseList {
  config: string;
  databases: DatabaseEntry[];
}
interface QueryResult {
  columns: string[];
  rows: (string | null)[][];
  row_count: number;
  truncated: boolean;
  elapsed_ms: number;
  sql?: string;
  database?: string;
}

let output: vscode.OutputChannel | undefined;
/** Share the extension's log channel so everything lands in one place. */
export function setDataLogChannel(channel: vscode.OutputChannel): void {
  output = channel;
}
function log(message: string): void {
  output?.appendLine(`[${new Date().toLocaleTimeString()}] ${message}`);
}

function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

/** Report an error the same way across every command in this file. */
async function reportError(e: unknown): Promise<void> {
  if (e instanceof BinaryNotFoundError) {
    vscode.window.showErrorMessage(e.message);
    return;
  }
  const message = e instanceof Error ? e.message : String(e);
  log(`error: ${message}`);
  // Engine errors carry their fix on a second paragraph; a toast truncates it,
  // so offer the full text rather than losing the useful half.
  const choice = await vscode.window.showErrorMessage(
    message.split("\n")[0],
    "Show details",
  );
  if (choice === "Show details") {
    const doc = await vscode.workspace.openTextDocument({
      content: message,
      language: "plaintext",
    });
    await vscode.window.showTextDocument(doc, { preview: true });
  }
}

// ── Connect wizard ──────────────────────────────────────────────────────────

/**
 * Ask for a connection, write it to `.fm-bridge.toml`, store the password
 * outside the project, and verify it — all without the user seeing a config
 * file or a command line.
 */
export async function connectCommand(): Promise<void> {
  const root = workspaceRoot();
  if (!root) {
    vscode.window.showErrorMessage(
      "Open a folder first: the connection is saved in that folder's .fm-bridge.toml.",
    );
    return;
  }

  const ask = async (
    prompt: string,
    placeHolder: string,
    value?: string,
    password = false,
  ): Promise<string | undefined> => {
    const v = await vscode.window.showInputBox({
      title: "Connect to a FileMaker database",
      prompt,
      placeHolder,
      value,
      password,
      ignoreFocusOut: true,
      validateInput: (s) => (s.trim().length === 0 ? "Required" : undefined),
    });
    return v?.trim();
  };

  const host = await ask(
    "Server address (the machine hosting the file)",
    "e.g. 10.0.0.5 or fms.example.com",
  );
  if (!host) return;

  const database = await ask(
    "FileMaker file name, without .fmp12",
    "e.g. Inventory",
  );
  if (!database) return;

  const user = await ask(
    "FileMaker account — use one with read-only access",
    "e.g. reporting",
  );
  if (!user) return;

  const password = await ask(
    `Password for "${user}". Stored outside this folder, never in the project.`,
    "",
    undefined,
    true,
  );
  if (!password) return;

  const serverLabel =
    (await ask(
      "A short label for this server",
      "e.g. production",
      "production",
    )) ?? "production";

  // Offer any FMSaveAsXML export in the workspace. This mapping is what lets
  // the engine relate live rows to the schema they came from.
  const xml = await pickXmlExport(root, database);

  const configPath = path.join(root, CONFIG_FILE);
  try {
    writeConfig(configPath, { serverLabel, host, user, database, xml });
  } catch (e) {
    await reportError(e);
    return;
  }

  try {
    await storePassword(serverLabel, password);
  } catch (e) {
    await reportError(e);
    return;
  }

  log(`connection "${database}" saved to ${configPath}`);
  await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "Testing the connection…",
    },
    async () => runDoctor(database),
  );
}

/** Let the user attach a FMSaveAsXML export to this database (optional). */
async function pickXmlExport(
  root: string,
  database: string,
): Promise<string | undefined> {
  const found = await vscode.workspace.findFiles(
    "**/*.xml",
    "**/node_modules/**",
    50,
  );
  if (found.length === 0) {
    return undefined;
  }
  const items: (vscode.QuickPickItem & { path?: string })[] = found.map(
    (u) => ({
      label: path.basename(u.fsPath),
      description: path.relative(root, u.fsPath),
      path: path.relative(root, u.fsPath).split(path.sep).join("/"),
    }),
  );
  // Put the likeliest match first — exports are usually named after the file.
  items.sort((a, b) => {
    const score = (i: (typeof items)[number]) =>
      i.label.toLowerCase().includes(database.toLowerCase()) ? 0 : 1;
    return score(a) - score(b);
  });
  items.push({
    label: "$(circle-slash) Skip",
    description: "No schema export for now",
  });

  const picked = await vscode.window.showQuickPick(items, {
    title: `Schema export for "${database}" (optional)`,
    placeHolder:
      "Pick the FMSaveAsXML export of this same file — it lets fm-bridge relate live data to the schema",
    ignoreFocusOut: true,
  });
  return picked?.path;
}

/** Append a server/database pair to `.fm-bridge.toml`, creating it if needed. */
function writeConfig(
  configPath: string,
  cfg: {
    serverLabel: string;
    host: string;
    user: string;
    database: string;
    xml?: string;
  },
): void {
  const existing = fs.existsSync(configPath)
    ? fs.readFileSync(configPath, "utf8")
    : "";

  const hasServer = hasNamedEntry(existing, "server", cfg.serverLabel);
  const hasDatabase = hasNamedEntry(existing, "database", cfg.database);

  let out = existing;
  if (out.length === 0) {
    out =
      "# fm-bridge live-data connections.\n" +
      "# Safe to commit: passwords are never stored here.\n";
  }
  if (!hasServer) {
    out +=
      `\n[[server]]\n` +
      `name = "${cfg.serverLabel}"\n` +
      `host = "${cfg.host}"\n` +
      `user = "${cfg.user}"\n`;
  }
  if (!hasDatabase) {
    out +=
      `\n[[database]]\n` +
      `name   = "${cfg.database}"\n` +
      `server = "${cfg.serverLabel}"\n` +
      `odbc   = "${cfg.database}"\n` +
      (cfg.xml ? `xml    = "${cfg.xml}"\n` : "");
  }
  if (out !== existing) {
    fs.writeFileSync(configPath, out, "utf8");
  }
}

/**
 * Is there already a `[[kind]]` block whose `name` is `name`?
 *
 * Split on table headers rather than scanning the whole file: a `[[server]]`
 * and a `[[database]]` may legitimately share a name, and blocks can appear in
 * any order, so a flat search would both miss duplicates and invent them.
 */
function hasNamedEntry(toml: string, kind: string, name: string): boolean {
  const blocks = toml.split(/^\s*\[\[/m).slice(1);
  const nameLine = new RegExp(
    `^\\s*name\\s*=\\s*"${escapeRe(name)}"\\s*$`,
    "m",
  );
  return blocks.some((b) => {
    const header = b.slice(0, b.indexOf("]]"));
    return header.trim() === kind && nameLine.test(b);
  });
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Hand the password to the binary over stdin — never as an argument, which
 * would put a production password in the OS process list.
 */
async function storePassword(server: string, password: string): Promise<void> {
  await spawnBinary(["data", "login", server], `${password}\n`);
}

// ── Doctor ──────────────────────────────────────────────────────────────────

export async function doctorCommand(): Promise<void> {
  try {
    const database = await pickDatabase("Diagnose which connection?", true);
    if (database === undefined) return;
    await runDoctor(database || undefined);
  } catch (e) {
    await reportError(e);
  }
}

/** Run the doctor and render every check with its fix. */
async function runDoctor(database?: string): Promise<void> {
  const resp = await runJson({
    command: "data_doctor",
    database,
    config_path: workspaceRoot(),
  });
  if (resp.status === "error") {
    await reportError(new Error(resp.error ?? "unknown error"));
    return;
  }
  const result = resp.data as DoctorResult;

  const lines = ["fm-bridge — live data diagnosis", ""];
  for (const c of result.checks) {
    lines.push(`${c.ok ? "PASS" : "FAIL"}  ${c.check}`);
    for (const l of c.detail.split("\n")) {
      lines.push(`      ${l}`);
    }
    lines.push("");
  }
  output?.appendLine(lines.join("\n"));

  if (result.ok) {
    vscode.window.showInformationMessage(
      `Connected. ${result.checks.filter((c) => c.ok).length} check(s) passed.`,
    );
    return;
  }
  const failed = result.checks.find((c) => !c.ok);
  const choice = await vscode.window.showErrorMessage(
    failed
      ? `${failed.check}: ${failed.detail.split("\n")[0]}`
      : "Diagnosis failed.",
    "How do I fix this?",
  );
  if (choice === "How do I fix this?") {
    const doc = await vscode.workspace.openTextDocument({
      content: lines.join("\n"),
      language: "plaintext",
    });
    await vscode.window.showTextDocument(doc, { preview: true });
  }
}

// ── Query ───────────────────────────────────────────────────────────────────

export async function queryCommand(): Promise<void> {
  try {
    const database = await pickDatabase("Query which database?", false);
    if (!database) return;

    const sql = await vscode.window.showInputBox({
      title: `Query ${database}`,
      prompt: "A SELECT statement. Read-only — writes are rejected.",
      placeHolder: 'SELECT "Name", "Qty" FROM "Inventory" WHERE "Qty" = 0',
      ignoreFocusOut: true,
      validateInput: (s) =>
        s.trim().length === 0 ? "Enter a SELECT statement" : undefined,
    });
    if (!sql) return;

    const resp = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `Querying ${database}…`,
      },
      async () =>
        runJson({
          command: "data_sql",
          database,
          sql,
          config_path: workspaceRoot(),
        }),
    );
    if (resp.status === "error") {
      await reportError(new Error(resp.error ?? "unknown error"));
      return;
    }
    await showResult(resp.data as QueryResult);
  } catch (e) {
    await reportError(e);
  }
}

/** Render a result set as an aligned table in a new editor tab. */
async function showResult(result: QueryResult): Promise<void> {
  const MAX_WIDTH = 40;
  const elide = (s: string) =>
    [...s].length > MAX_WIDTH
      ? [...s].slice(0, MAX_WIDTH - 1).join("") + "…"
      : s;

  const header = result.columns.map(elide);
  const rows = result.rows.map((r) => r.map((c) => elide(c ?? "<null>")));
  const widths = header.map((h, i) =>
    Math.max([...h].length, ...rows.map((r) => [...(r[i] ?? "")].length), 0),
  );
  const line = (cells: string[]) =>
    cells.map((c, i) => c.padEnd(widths[i] ?? 0)).join(" | ");

  const body = [
    `-- ${result.database ?? ""}`,
    `-- ${result.sql ?? ""}`,
    "",
    line(header),
    widths.map((w) => "-".repeat(w)).join("-+-"),
    ...rows.map(line),
    "",
    `${result.row_count} row(s) in ${result.elapsed_ms} ms` +
      (result.truncated
        ? "   [TRUNCATED — a size limit was reached; this is NOT the full result]"
        : ""),
  ].join("\n");

  const doc = await vscode.workspace.openTextDocument({
    content: body,
    language: "plaintext",
  });
  await vscode.window.showTextDocument(doc, { preview: false });
}

// ── Shared ──────────────────────────────────────────────────────────────────

/**
 * Pick one of the configured databases. Returns `""` for "all" when
 * `allowAll`, `undefined` when the user cancels.
 */
async function pickDatabase(
  title: string,
  allowAll: boolean,
): Promise<string | undefined> {
  const resp = await runJson({
    command: "data_databases",
    config_path: workspaceRoot(),
  });
  if (resp.status === "error") {
    const choice = await vscode.window.showErrorMessage(
      resp.error ?? "No connection configured.",
      "Connect to a database…",
    );
    if (choice) {
      await connectCommand();
    }
    return undefined;
  }
  const list = resp.data as DatabaseList;
  if (list.databases.length === 0) {
    const choice = await vscode.window.showErrorMessage(
      "No databases configured yet.",
      "Connect to a database…",
    );
    if (choice) {
      await connectCommand();
    }
    return undefined;
  }

  const items: (vscode.QuickPickItem & { value: string })[] =
    list.databases.map((d) => ({
      label: d.name,
      description: `${d.server} · ${d.odbc_name}`,
      detail: d.xml ? `schema: ${d.xml}` : undefined,
      value: d.name,
    }));
  if (allowAll && items.length > 1) {
    items.unshift({ label: "All databases", value: "" });
  }
  if (items.length === 1 && !allowAll) {
    return items[0].value;
  }
  const picked = await vscode.window.showQuickPick(items, {
    title,
    ignoreFocusOut: true,
  });
  return picked?.value;
}
