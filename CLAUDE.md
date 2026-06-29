# CLAUDE.md

Context for AI assistants working in this repo. Read [docs/VISION.md](docs/VISION.md)
first — it's the north star (where the project is going). This file is the quick
operational context.

## What fm-bridge is

A Rust CLI that moves FileMaker scripts between the clipboard and plain-text
`.fmscript` files, so they can be edited outside FileMaker's Script Workspace.
It decodes/encodes FileMaker's proprietary clipboard XML (`fmxmlsnippet`).

## North star (see docs/VISION.md)

Two front doors over **one engine**: a **human** path (VS Code extension editing
`.fmscript`) and an **AI** path (future MCP server). Both are thin clients of the
Rust binary; both share the same artifacts (`.fmscript` text + JSON), so they're
interchangeable.

## Architecture

- **The binary is the single source of truth.** All logic — parser, linter,
  XML↔text codec, step catalog, future schema inspect — lives in `src/`.
  Never duplicate this logic in a client (extension, MCP).
- **Clients are thin.** The VS Code extension (`editors/vscode/`) just calls the
  binary (`fm-bridge json` over stdin/stdout, and `fm-bridge steps`).
- **Zero-install for end users.** The binary is bundled inside the `.vsix`
  (`editors/vscode/bin/<platform>-<arch>/`); the extension prefers it over any
  local install. Installing the `.vsix` is all an end user needs — no Rust.

## Key files

- `src/main.rs` — CLI + JSON-protocol dispatch.
- `src/xmss.rs` — FileMaker XML codec (decode/encode). Only understands `<Step>`
  (scripts) today; schema is greenfield (#6).
- `src/text_format.rs` — `.fmscript` parser/formatter + `lint` (the validator
  behind diagnostics).
- `src/steps.rs` + `steps.toml` — the step catalog (single source for names,
  shapes, block behavior). `steps.toml` is the data; add steps there.
- `src/fmsavexml.rs` + `src/slice.rs` — schema parser (`inspect`/`slice`, #6):
  FMSaveAsXML → navigable dirs of tables/fields/layouts/TOs/relations/scripts.
- `src/audit.rs` — referential-integrity audit (`audit`, Phase 3): crosses
  scripts × schema to flag broken references (dangling Perform Script / Go to
  Layout, relationships/layouts to missing TOs, ghost fields).
- `src/xref.rs` — cross-reference queries (`who-calls` / `who-uses-field`):
  reverse call graph + field-usage search over the parsed database.
- `src/mcp.rs` — the **AI front door**: `fm-bridge mcp` serves MCP (JSON-RPC over
  stdio), exposing engine commands as tools. Forwards to `handle_command`.
- `editors/vscode/` — the VS Code extension (TypeScript).
- `docs/VISION.md` — north star. `docs/MCP.md` — MCP setup + tools.
  `docs/USAGE.md` — user guide (ES). `docs/IA-PROMPT.md` — `.fmscript` syntax for AI.

## Build & test

```bash
cargo test                       # Rust tests
cargo build --release            # build the binary
cargo install --path .           # install to ~/.cargo/bin

cd editors/vscode
npm install
npm run package:bundled          # build native binary + self-contained .vsix
npm run typecheck
```

A universal multiplatform `.vsix` is produced by the `Package extension` GitHub
workflow on a `v*` tag.

## Conventions

- **JSON protocol is a stable contract** (the extension depends on it). New
  fields must be optional with `#[serde(skip_serializing_if = ...)]`.
- **Don't run a project-wide `cargo fmt`.** The CI `lint` job is intentionally
  non-blocking; the codebase isn't rustfmt/clippy-clean yet and a global reformat
  collides with in-flight work in `text_format.rs`. Match the file's existing style.
- **Opaque by default:** anything the codec doesn't model structurally must
  round-trip verbatim (see #2). Don't drop unknown data.
- Steps are identified by exact name; the catalog lives in `steps.toml`.
