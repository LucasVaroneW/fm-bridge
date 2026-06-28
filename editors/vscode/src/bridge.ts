// Thin wrapper around the fm-bridge Rust binary.
//
// Two transport shapes:
//   - JSON mode (`fm-bridge json`): reads one JSON command on stdin, writes one
//     JSON response on stdout. Used for read / write / parse / version.
//   - Subcommand mode (`fm-bridge steps`): prints the step catalog as JSON to
//     stdout. Used for autocomplete.
//
// The binary is the single source of truth — the extension never duplicates the
// step catalog or the parser. If the binary is missing we surface a clear,
// actionable error rather than guessing.

import * as cp from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

/** One validation error: a 1-based line and a message. */
export interface BridgeError {
  line: number;
  message: string;
}

/** Shape of the JSON-mode response (see `Response` in src/main.rs). */
export interface BridgeResponse {
  status: "ok" | "error";
  script_text?: string;
  error?: string;
  version?: string;
  /** 1-based source line of the first error (mirrors errors[0]). */
  error_line?: number;
  /** All validation errors found by the linter, when present. */
  errors?: BridgeError[];
}

/** One entry of the step catalog (see `StepInfo` in src/steps.rs). */
export interface StepInfo {
  en: string;
  es: string;
  shape: string;
  opens_block: boolean;
  closes_block: boolean;
  has_id: boolean;
}

/** Thrown when the binary cannot be located. Caught at the command boundary. */
export class BinaryNotFoundError extends Error {
  constructor() {
    super(
      "fm-bridge binary not found. Install it with `cargo install --path .` " +
        "or set fmBridge.binaryPath in Settings.",
    );
    this.name = "BinaryNotFoundError";
  }
}

function expandHome(p: string): string {
  if (p === "~") {
    return os.homedir();
  }
  if (p.startsWith("~/") || p.startsWith("~\\")) {
    return path.join(os.homedir(), p.slice(2));
  }
  return p;
}

/** Scan the PATH env var for an executable, returning the first hit. */
function findOnPath(exe: string): string | undefined {
  const pathVar = process.env.PATH ?? "";
  for (const dir of pathVar.split(path.delimiter)) {
    if (!dir) {
      continue;
    }
    const candidate = path.join(dir, exe);
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

let cachedAutodetect: string | undefined;

/**
 * Resolve the fm-bridge binary path. Order:
 *   1. fmBridge.binaryPath setting (trusted as-is, ~ expanded).
 *   2. ~/.cargo/bin/fm-bridge[.exe]  (the documented install location).
 *   3. fm-bridge[.exe] anywhere on PATH.
 * Returns undefined if none resolve.
 */
export function resolveBinaryPath(): string | undefined {
  const configured = vscode.workspace
    .getConfiguration("fmBridge")
    .get<string>("binaryPath");
  if (configured && configured.trim().length > 0) {
    return expandHome(configured.trim());
  }

  if (cachedAutodetect && fs.existsSync(cachedAutodetect)) {
    return cachedAutodetect;
  }

  const exe = process.platform === "win32" ? "fm-bridge.exe" : "fm-bridge";
  const cargoBin = path.join(os.homedir(), ".cargo", "bin", exe);
  if (fs.existsSync(cargoBin)) {
    cachedAutodetect = cargoBin;
    return cargoBin;
  }

  const onPath = findOnPath(exe);
  if (onPath) {
    cachedAutodetect = onPath;
    return onPath;
  }

  return undefined;
}

/** Clear the cached autodetected path (call when the setting changes). */
export function resetBinaryCache(): void {
  cachedAutodetect = undefined;
}

function spawnBinary(args: string[], stdin?: string): Promise<string> {
  const bin = resolveBinaryPath();
  if (!bin) {
    return Promise.reject(new BinaryNotFoundError());
  }
  return new Promise((resolve, reject) => {
    const child = cp.execFile(
      bin,
      args,
      { maxBuffer: 64 * 1024 * 1024, windowsHide: true },
      (err, stdout, stderr) => {
        // JSON mode exits 0 even for `{"status":"error"}`, so a non-empty
        // stdout is authoritative. Only treat as failure when there's nothing
        // to parse.
        if (stdout && stdout.trim().length > 0) {
          resolve(stdout);
          return;
        }
        if (err) {
          const code = (err as NodeJS.ErrnoException).code;
          if (code === "ENOENT") {
            reject(new BinaryNotFoundError());
            return;
          }
          reject(new Error(stderr?.trim() || err.message));
          return;
        }
        resolve(stdout);
      },
    );
    if (child.stdin) {
      child.stdin.end(stdin ?? "");
    }
  });
}

/** Run a JSON-mode command and parse the response. */
async function runJson(command: Record<string, unknown>): Promise<BridgeResponse> {
  const out = await spawnBinary(["json"], JSON.stringify(command));
  try {
    return JSON.parse(out) as BridgeResponse;
  } catch {
    throw new Error(`Unexpected output from fm-bridge: ${out.slice(0, 200)}`);
  }
}

/** Read the FileMaker clipboard and return the decoded .fmscript text. */
export async function readClipboard(): Promise<BridgeResponse> {
  return runJson({ command: "read" });
}

/** Encode the given text and write it to the FileMaker clipboard. */
export async function writeClipboard(scriptText: string): Promise<BridgeResponse> {
  return runJson({ command: "write", script_text: scriptText });
}

/** Validate text without any clipboard side effect. Drives diagnostics. */
export async function parseScript(scriptText: string): Promise<BridgeResponse> {
  return runJson({ command: "parse", script_text: scriptText });
}

/** Fetch the binary's version (also used as a reachability probe). */
export async function version(): Promise<BridgeResponse> {
  return runJson({ command: "version" });
}

/** Fetch the full step catalog (single source of truth for autocomplete). */
export async function steps(): Promise<StepInfo[]> {
  const out = await spawnBinary(["steps"]);
  try {
    return JSON.parse(out) as StepInfo[];
  } catch {
    throw new Error(`Unexpected step catalog from fm-bridge: ${out.slice(0, 200)}`);
  }
}
