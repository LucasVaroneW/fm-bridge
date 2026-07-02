# fm-bridge — VS Code extension

Edit **FileMaker scripts as plain-text `.fmscript` files** in VS Code, then move
them in and out of FileMaker through the clipboard. The extension is a thin layer
over the `fm-bridge` Rust binary (see [docs/USAGE.md](../../docs/USAGE.md)), which
does all the FileMaker-specific encoding/decoding — so the editor never drifts
from what the binary actually supports.

## Install (recommended — no build, no Rust)

Download the latest packaged extension and install it. The binary for your OS is
already inside the `.vsix`, so this is all you need:

1. **Download** `fm-bridge-<version>.vsix` from the latest release:
   **<https://github.com/LucasVaroneW/fm-bridge/releases/latest>**
   (it's under **Assets**).
2. **Install** it in VS Code:
   `Cmd/Ctrl+Shift+P` → **Extensions: Install from VSIX…** → pick the file
   (or: **Extensions** panel → `…` menu → **Install from VSIX…**).
3. **Reload** the window: `Cmd/Ctrl+Shift+P` → **Developer: Reload Window**.

That's it — no Rust, no npm. To check it loaded, open the Command Palette and
type `fm-bridge`; you should see the commands listed below. Upgrading later is
the same steps with a newer `.vsix` (VS Code replaces the old version).

> Using **Antigravity** or another VS Code fork? Same step 2 from the `…` menu →
> **Install from VSIX…**.

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
| `fm-bridge.formatInline` | fm-bridge: Format inline (1 line per step, matches FileMaker) |
| `fm-bridge.formatIndented` | fm-bridge: Format indented (readable multi-line) |
| `fm-bridge.showLog` | fm-bridge: Show log |

**Inline vs indented:** multi-field steps (Import Records, Commit Records, Go to
Related Record) render across several lines for readability. That makes a
`.fmscript` line number drift from FileMaker's, where every step is one line.
**Format inline** collapses each such step back to a single line so the numbers
line up 1:1 (handy when chasing "line 1500" across both); **Format indented**
restores the readable view.

Multi-line **calculations** (a `Set Variable` with a long `Let(…)`, an `If`
calc, etc.) are also collapsed by **Format inline**. Because a FileMaker `//`
comment runs to the end of its line, collapsing to one line would make it
swallow whatever follows — so inline rewrites each `// note` as the equivalent
`/* note */` block comment. The calculation stays semantically identical and
still pastes correctly; the one caveat is that this `//`→`/* */` change isn't
reversed when you go back to indented (the comment stays a block comment).

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

## Publish a release (maintainers)

To cut a version everyone can install **without building** (the universal `.vsix`
that works on macOS arm64/x64, Windows and Linux):

1. Bump the version in `editors/vscode/package.json` (and `Cargo.toml`, to keep
   the binary's reported version in sync), commit, push `main`.
2. Tag it and push the tag:
   ```bash
   git tag v0.1.1
   git push origin v0.1.1
   ```
3. The **Package extension** workflow fires on the `v*` tag: it builds each
   platform's binary, bundles them all into one `.vsix`, and **attaches it to a
   GitHub Release** for that tag. ~3 minutes later it's at
   <https://github.com/LucasVaroneW/fm-bridge/releases/latest>.
4. Share that link — collaborators just download and install (see **Install** at
   the top). No Rust, no npm on their side.

Install the resulting `.vsix`:

```bash
code --install-extension fm-bridge-0.1.5.vsix
```

or in VS Code: **Extensions** panel → `…` menu → **Install from VSIX…**

### Installing in Antigravity (or other VS Code forks)

Antigravity is a VS Code fork, so the same `.vsix` works there too — but its
`code`-style CLI binary is named `antigravity-ide`, not `code`:

```bash
"/Applications/Antigravity IDE.app/Contents/Resources/app/bin/antigravity-ide" \
  --install-extension fm-bridge-0.1.5.vsix
```

or from the GUI: **Extensions** panel → `…` menu → **Install from VSIX…**.

To iterate: open `editors/vscode` in VS Code and press **F5** (Run Extension) —
or run `npm run watch` and reload the Extension Development Host.
