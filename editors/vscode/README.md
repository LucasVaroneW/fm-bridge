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

The `fm-bridge` binary must be installed. From the repo root:

```bash
cargo install --path .
```

That puts `fm-bridge` in `~/.cargo/bin`, which the extension autodetects. If your
binary lives elsewhere, set **`fmBridge.binaryPath`** in Settings to its absolute
path (a leading `~` is expanded).

## Commands

| Command | Palette title |
|---|---|
| `fm-bridge.readFromClipboard` | fm-bridge: Read script from clipboard |
| `fm-bridge.writeToClipboard` | fm-bridge: Write script to clipboard |

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

## Build from source

```bash
cd editors/vscode
npm install
npm run build        # bundle to dist/extension.js
npm run package      # produce fm-bridge-<version>.vsix
```

Install the resulting `.vsix`:

```bash
code --install-extension fm-bridge-0.1.0.vsix
```

or in VS Code: **Extensions** panel → `…` menu → **Install from VSIX…**

To iterate: open `editors/vscode` in VS Code and press **F5** (Run Extension) —
or run `npm run watch` and reload the Extension Development Host.
