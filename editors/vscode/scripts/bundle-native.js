// Build the fm-bridge Rust binary for THIS machine and copy it into the
// extension at bin/<platform>-<arch>/, where resolveBinaryPath() looks for it.
// Used for local self-contained packaging; CI assembles all platforms instead.
//
// The ODBC sidecar rides along when it is available. It is best-effort on
// purpose: it is the only component that links a native ODBC driver manager, so
// a machine without one still produces a working .vsix — schema tools are
// unaffected and the live-data path reports itself unavailable.
const cp = require("child_process");
const fs = require("fs");
const path = require("path");

const repoRoot = path.resolve(__dirname, "..", "..", "..");
const isWindows = process.platform === "win32";
const exe = isWindows ? "fm-bridge.exe" : "fm-bridge";
const sidecar = isWindows ? "fm-bridge-odbc.exe" : "fm-bridge-odbc";

const destDir = path.join(
  __dirname,
  "..",
  "bin",
  `${process.platform}-${process.arch}`,
);
fs.mkdirSync(destDir, { recursive: true });

function copyInto(src, name) {
  const dest = path.join(destDir, name);
  fs.copyFileSync(src, dest);
  if (!isWindows) {
    fs.chmodSync(dest, 0o755);
  }
  console.log(`Bundled → ${path.relative(path.join(__dirname, ".."), dest)}`);
}

console.log("Building fm-bridge (cargo build --release)…");
cp.execSync("cargo build --release", { cwd: repoRoot, stdio: "inherit" });
copyInto(path.join(repoRoot, "target", "release", exe), exe);

console.log("Building the ODBC sidecar (optional)…");
try {
  cp.execSync("cargo build --release -p fm-bridge-odbc", {
    cwd: repoRoot,
    stdio: "inherit",
  });
  copyInto(path.join(repoRoot, "target", "release", sidecar), sidecar);
} catch (e) {
  console.warn(
    `\n⚠  ODBC sidecar not bundled: ${e.message}\n` +
      "   Live data queries will be unavailable in this .vsix. Everything that\n" +
      "   reads scripts and FMSaveAsXML exports still works.\n",
  );
}

// The alternate-architecture sidecar, if it was cross-compiled beforehand.
// On Windows this is the important one: FileMaker Pro commonly registers only
// the 32-bit ODBC driver, which a 64-bit process cannot load.
const altBuilds = isWindows
  ? ["i686-pc-windows-msvc", "i686-pc-windows-gnu"]
  : ["x86_64-apple-darwin"];
for (const target of altBuilds) {
  const src = path.join(repoRoot, "target", target, "release", sidecar);
  if (fs.existsSync(src)) {
    copyInto(src, isWindows ? "fm-bridge-odbc-x86.exe" : "fm-bridge-odbc-x86");
    break;
  }
}
