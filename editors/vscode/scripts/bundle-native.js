// Build the fm-bridge Rust binary for THIS machine and copy it into the
// extension at bin/<platform>-<arch>/, where resolveBinaryPath() looks for it.
// Used for local self-contained packaging; CI assembles all platforms instead.
const cp = require("child_process");
const fs = require("fs");
const path = require("path");

const repoRoot = path.resolve(__dirname, "..", "..", "..");
const exe = process.platform === "win32" ? "fm-bridge.exe" : "fm-bridge";

console.log("Building fm-bridge (cargo build --release)…");
cp.execSync("cargo build --release", { cwd: repoRoot, stdio: "inherit" });

const src = path.join(repoRoot, "target", "release", exe);
const destDir = path.join(
  __dirname,
  "..",
  "bin",
  `${process.platform}-${process.arch}`,
);
fs.mkdirSync(destDir, { recursive: true });
const dest = path.join(destDir, exe);
fs.copyFileSync(src, dest);
if (process.platform !== "win32") {
  fs.chmodSync(dest, 0o755);
}
console.log(`Bundled → ${path.relative(path.join(__dirname, ".."), dest)}`);
