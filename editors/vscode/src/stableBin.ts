// A version-independent home for the binaries.
//
// The bundled binary lives in the extension folder, whose name carries the
// version: `lucasvarone.fm-bridge-0.1.8/bin/…`. Any MCP config pointing there
// breaks on the next extension update, because VS Code installs into a *new*
// folder and removes the old one. The AI client then fails to start the server
// with a bare "file not found", which looks like the tool is broken.
//
// So we mirror the binaries into `~/.fm-bridge/bin/`, refresh that copy when
// the extension updates, and point MCP clients at the stable path. Configured
// once, correct forever.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";

/** `~/.fm-bridge/bin` — stable across extension versions. */
export function stableBinDir(): string {
  return path.join(os.homedir(), ".fm-bridge", "bin");
}

const EXE = process.platform === "win32" ? ".exe" : "";
/** The engine plus every sidecar that may sit beside it. */
const BINARIES = [
  `fm-bridge${EXE}`,
  `fm-bridge-odbc${EXE}`,
  `fm-bridge-odbc-x86${EXE}`,
];

/** Same size and mtime is close enough to skip a copy on every activation. */
function upToDate(src: string, dest: string): boolean {
  try {
    const a = fs.statSync(src);
    const b = fs.statSync(dest);
    return a.size === b.size && Math.abs(a.mtimeMs - b.mtimeMs) < 1000;
  } catch {
    return false;
  }
}

/**
 * Mirror the bundled binaries into the stable directory and return the path of
 * the engine there.
 *
 * `bundledPath` is the engine inside the extension; sidecars are picked up from
 * the same folder. Returns undefined when the mirror cannot be established, so
 * callers can fall back to the bundled path rather than fail.
 */
export function ensureStableBinaries(bundledPath: string): string | undefined {
  const srcDir = path.dirname(bundledPath);
  const destDir = stableBinDir();
  try {
    fs.mkdirSync(destDir, { recursive: true });
  } catch {
    return undefined;
  }

  let engine: string | undefined;
  for (const name of BINARIES) {
    const src = path.join(srcDir, name);
    const dest = path.join(destDir, name);
    if (!fs.existsSync(src)) {
      // A sidecar may legitimately be missing (a platform without one). Never
      // leave a stale copy behind: it would claim a capability we cannot serve.
      if (fs.existsSync(dest)) {
        try {
          fs.rmSync(dest);
        } catch {
          /* best effort */
        }
      }
      continue;
    }
    try {
      if (!upToDate(src, dest)) {
        fs.copyFileSync(src, dest);
        const { atime, mtime } = fs.statSync(src);
        fs.utimesSync(dest, atime, mtime);
        if (process.platform !== "win32") {
          fs.chmodSync(dest, 0o755);
        }
      }
      if (name === `fm-bridge${EXE}`) {
        engine = dest;
      }
    } catch {
      // A locked file (the MCP server is running this very binary) is the
      // common case. An existing copy is still usable.
      if (name === `fm-bridge${EXE}` && fs.existsSync(dest)) {
        engine = dest;
      }
    }
  }
  return engine;
}
