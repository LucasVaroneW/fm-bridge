# fm-bridge

Mueve scripts de FileMaker entre el clipboard y archivos `.fmscript` de texto
plano. Un solo motor Rust con dos puertas: **humana** (VS Code) y **IA** (MCP).

## Arquitectura

```
                 ┌─ fm-bridge json ──▶ Extensión VS Code   (lectura/escritura,
                 │                       diagnostics, autocomplete)
fm-bridge.exe ──┤
  (motor Rust)  └─ fm-bridge mcp ──▶ Cliente MCP           (tools para IA:
                                      (Claude, OpenCode…)    read, validate,
                                                            inspect, audit…)
```

El binario (`src/`) es la única fuente de verdad. La extensión y el MCP son
clientes finos que le hablan por stdin/stdout (JSON) — nunca duplican lógica.

## Estructura del motor

```
src/
├── main.rs           entrypoint, dispatch CLI + protocolo JSON
├── xmss.rs           codec del XML de clipboard de FM (decode/encode)
├── text_format.rs    parser/formatter del formato .fmscript + linter
├── steps.rs          catálogo de pasos (cargado de steps.toml en compile-time)
├── step_dsl.rs       DSL para pasos opacos (Go to Related Record, etc.)
├── import_records.rs DSL para bloques Import/Export Records
├── clipboard.rs      I/O al clipboard (Windows OLE + macOS NSPasteboard)
├── ole_clipboard.rs  clipboard OLE para Windows
├── normalization.rs  normalización Unicode (nombres de paso)
├── fmsavexml.rs      parser del export FMSaveAsXML (inspect, describe, get_*)
├── slice.rs          slice enfocado desde un inspect (cierre transitivo)
├── audit.rs          auditoría de integridad referencial (Perform Script,
│                     Go to Layout rotos, TOs huérfanas, campos fantasma)
├── xref.rs           referencias cruzadas (who-calls, who-uses-field)
└── mcp.rs            servidor MCP (JSON-RPC por stdin/stdout)

steps.toml            catálogo de ~150 pasos FM: nombre EN/ES, id numérico,
                      shape (cómo se serializa en XML), block behavior
```

## Formato `.fmscript`

Un paso por línea, indentación de 2 espacios para bloques (If/Loop):

```
Set Error Capture [True]
Loop
  Set Variable [$i = $i + 1]
  Exit Loop If [$i >= 10]
End Loop
Go to Layout ["Ta_Stock" #2613]
```

Las shapes determinan cómo se escribe el contenido entre `[]`. Por ejemplo:
- `Set Field [Table::Field; value]`
- `New Window [Style: Document; Layout: "X" #2888]`
- `Revert Transaction [Condition: calc; ErrorCode: 111]`

Los pasos con shape `plain` no llevan `[]` (ej: `Halt Script`, `End If`).
Los pasos con shape `opaque` preservan el XML interno palabra por palabra.

## Protocolo JSON

La extensión y el MCP usan el mismo protocolo por stdin/stdout:

```json
// Request
{"command": "read"}
{"command": "write", "script_text": "Set Variable [$x = 1]"}
{"command": "parse", "script_text": "..."}
{"command": "steps"}

// Response
{"status": "ok", "script_text": "..."}
{"status": "error", "error": "Unknown step...", "error_line": 3,
 "errors": [{"line": 3, "message": "...", "severity": "error"}]}
```

Campos nuevos deben ser opcionales con `#[serde(skip_serializing_if = ...)]`.

## Comandos CLI

| Comando | Descripción |
|---|---|
| `read [file]` | Lee clipboard FM → `.fmscript` |
| `write <file>` | `.fmscript` → clipboard FM |
| `json` | Modo protocolo JSON (stdin/stdout) |
| `mcp` | Servidor MCP (JSON-RPC por stdio) |
| `steps` | Catálogo de pasos en JSON |
| `debug` | Vuelca `debug_raw.xml` y `debug_built.xml` |
| `dump-ids` | Lista `id<TAB>nombre` de pasos en clipboard |
| `inspect <xml> [dir]` | Parsea FMSaveAsXML → directorio navegable |
| `slice <out> <slice> <layouts...>` | Slice enfocado por layouts |
| `audit <xml>` | Busca referencias rotas |
| `who-calls <xml> <script>` | Qué dispara un script |
| `who-uses-field <xml> <field>` | Dónde se usa un campo |
| `describe <xml>` | Resumen inline de la base |
| `get-table <xml> <table>` | Campos de una tabla (inline) |
| `get-script <xml> <name\|#id>` | Un script (inline) |
| `get-layout <xml> <name\|#id>` | Un layout (inline) |
| `encode-text <in> <out>` | `.fmscript` → XML |
| `decode-xml <in>` | XML → `.fmscript` |
| `passthrough` | Clipboard → clipboard (sin modificar) |

## [Release](docs/RELEASE.md)

Ver `docs/RELEASE.md` para el flujo de build y empaquetado.
