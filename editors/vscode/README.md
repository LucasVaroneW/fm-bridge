# fm-bridge — VS Code extension

Edit **FileMaker scripts as plain-text `.fmscript` files** in VS Code, then move
them in and out of FileMaker through the clipboard. The extension is a thin layer
over the `fm-bridge` Rust binary (see [docs/USAGE.md](../../docs/USAGE.md)), which
does all the FileMaker-specific encoding/decoding — so the editor never drifts
from what the binary actually supports.

## Features

- **Syntax highlighting** for `.fmscript` — step names, `# comments`,
  `// disabled` steps, `[ ... ]` parameters, `"strings"`, `$variables`, calc
  functions and operators.
- **Snippets** for common steps (`setv`, `setf`, `if`, `ifelse`, `loop`,
  `dialog`, `perform`, `find`, `inserturl`, …).
- **Read from clipboard** — copy a script in FileMaker, run the command, and the
  decoded `.fmscript` opens in a new editor.
- **Write to clipboard** — encodes the active `.fmscript` and puts it on the
  clipboard in FileMaker's format; paste it into the Script Workspace.
- **Diagnostics** — format errors are underlined on the correct line as you type
  (debounced) and on save. Validation never touches your clipboard.
- **Autocomplete** — type a step name and get the full step with its parameter
  template. The list comes straight from `fm-bridge steps`, so it always matches
  the installed binary.

## Requirements

**None for the released `.vsix`** — the `fm-bridge` binary ships *inside* the
extension, so installing the `.vsix` gives you everything. No Rust, no separate
install. The extension picks the right binary for your OS/arch at runtime.

How it finds the binary, in order:

1. `fmBridge.binaryPath` setting (if you set it — a leading `~` is expanded).
2. The bundled binary (`bin/<platform>-<arch>/` inside the extension).
3. `~/.cargo/bin/fm-bridge` (handy when developing from source).
4. `fm-bridge` anywhere on `PATH`.

> If you build the extension yourself, the bundled binary is produced by
> `npm run bundle:native` (your platform) or by CI (all platforms). See below.

## Commands

| Command | Palette title |
|---|---|
| `fm-bridge.readFromClipboard` | fm-bridge: Read script from clipboard |
| `fm-bridge.writeToClipboard` | fm-bridge: Write script to clipboard |
| `fm-bridge.inspectXml` | fm-bridge: Inspect FMSaveAsXML export |
| `fm-bridge.slice` | fm-bridge: Slice inspect output around layouts |
| `fm-bridge.copyMcpConfig` | fm-bridge: Set up MCP for an AI agent |
| `fm-bridge.showLog` | fm-bridge: Show log |

`Write` is also available as a button in the editor title bar for `.fmscript`
files.

## Settings

| Setting | Default | Description |
|---|---|---|
| `fmBridge.binaryPath` | `""` | Absolute path to the `fm-bridge` binary. Empty = autodetect (`~/.cargo/bin`, then `PATH`). |
| `fmBridge.validateOnType` | `true` | Validate as you type. Set `false` to validate only on save. |

## Typical workflow

1. In FileMaker: select script steps, `Cmd/Ctrl+C`.
2. **fm-bridge: Read script from clipboard** → edit the `.fmscript` in VS Code.
3. **fm-bridge: Write script to clipboard**.
4. In FileMaker: `Cmd/Ctrl+V` into the Script Workspace.

## Use it from an AI agent (MCP)

The same binary that powers this extension is also an **MCP server** — the door
an AI agent (OpenCode, Claude Desktop, Cursor, the VS Code agent…) uses to drive
the engine: inspect a database, audit broken references, pull one table or one
script, etc. See [docs/MCP.md](../../docs/MCP.md) for the full tool list.

**Important:** installing the `.vsix` gives you the human editor **and the
binary on disk**, but it does **not** auto-configure the MCP in your AI client —
that client is a separate app with its own config file. You just have to point it
at the binary once. The extension makes that one step painless:

1. Run **fm-bridge: Set up MCP for an AI agent** from the Command Palette.
2. Pick your client (OpenCode / Claude Desktop / Cursor).
3. Choose **Apply to <client>'s config** — the extension finds that client's
   config file for your OS and **writes/merges the MCP entry itself** (with the
   real bundled-binary path filled in, and a `.bak` backup of the original). Or
   pick **Copy to clipboard instead** for the manual route.
4. Restart the AI client. Done — no Rust, no hand-typed paths.

> Already have a capable agent (this one, OpenCode, Claude Code)? You can skip
> the command entirely and just ask it: *"set up the fm-bridge MCP in OpenCode"* —
> it edits the config file for you. The command is the universal fallback for
> when you don't yet have an agent that can edit files.

After that, in a fresh chat you don't mention "MCP" at all: just give the agent a
path and a task (e.g. *"inspect `C:\exports\ventas.xml` and tell me if any Set
Field points at a missing field"*) and it picks the right tool on its own.

## Build from source

```bash
cd editors/vscode
npm install
npm run package:bundled   # build the native binary + produce a self-contained .vsix
```

`package:bundled` runs `cargo build --release`, copies the binary into
`bin/<your-platform>/`, and packages it — so the resulting `.vsix` works on your
machine with no separate install. (`npm run package` alone skips the binary and
relies on `~/.cargo/bin`/`PATH`.)

For a **universal** `.vsix` that works on macOS (arm64/x64), Windows and Linux,
push a `v*` tag (or run the *Package extension* workflow): CI builds every
platform's binary, bundles them all, and attaches the `.vsix` to the release.

Install the resulting `.vsix`:

```bash
code --install-extension fm-bridge-0.1.1.vsix
```

or in VS Code: **Extensions** panel → `…` menu → **Install from VSIX…**

### Installing in Antigravity (or other VS Code forks)

Antigravity is a VS Code fork, so the same `.vsix` works there too — but its
`code`-style CLI binary is named `antigravity-ide`, not `code`:

```bash
"/Applications/Antigravity IDE.app/Contents/Resources/app/bin/antigravity-ide" \
  --install-extension fm-bridge-0.1.1.vsix
```

or from the GUI: **Extensions** panel → `…` menu → **Install from VSIX…**.

To iterate: open `editors/vscode` in VS Code and press **F5** (Run Extension) —
or run `npm run watch` and reload the Extension Development Host.
