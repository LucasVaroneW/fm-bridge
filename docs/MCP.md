# fm-bridge MCP server — la puerta IA

`fm-bridge mcp` levanta un servidor **MCP (Model Context Protocol)** por stdio.
Es la **puerta IA**: cualquier cliente MCP (Claude Desktop, Cursor, Antigravity,
Zed…) maneja el **mismo motor** que usa el humano, porque cada tool reenvía a un
comando del binario — no se duplica lógica.

No necesitás Node ni nada extra: es un subcomando del mismo binario que ya viene
empaquetado en el `.vsix` (o instalado en `~/.cargo/bin`).

## Cómo se configura

El cliente lanza `fm-bridge mcp` y habla JSON-RPC por stdin/stdout. Apuntá al
binario con ruta absoluta.

### Claude Desktop

`claude_desktop_config.json`
(macOS: `~/Library/Application Support/Claude/`, Windows: `%APPDATA%\Claude\`):

```json
{
  "mcpServers": {
    "fm-bridge": {
      "command": "/Users/TU_USUARIO/.cargo/bin/fm-bridge",
      "args": ["mcp"]
    }
  }
}
```

En Windows, `command` apunta al `.exe`, p. ej.
`C:\\Users\\TU_USUARIO\\.cargo\\bin\\fm-bridge.exe`.

> ¿No tenés el binario suelto? Está empaquetado dentro de la extensión, en
> `~/.vscode/extensions/lucasvarone.fm-bridge-*/bin/<plataforma>/fm-bridge`.
> Podés apuntar ahí, o instalarlo con `cargo install --path .`.

### Cursor / Antigravity / otros

Mismo patrón: en su archivo de MCP servers, comando = ruta al binario,
args = `["mcp"]`.

## Tools que expone

| Tool | Qué hace | Args |
|---|---|---|
| `read_clipboard_script` | Lee el clipboard de FM → `.fmscript` | — |
| `validate_script` | Linter: todos los errores de formato/estructura | `script_text` |
| `script_to_json` | Árbol estructurado del script (steps, calcs, campos) | `script_text` |
| `inspect_database` | Parsea un export `FMSaveAsXML` → directorio navegable + conteos | `xml_path`, `output_dir?` |
| `slice_inspect` | Subconjunto enfocado por layout (cierre transitivo) | `output_dir`, `slice_dir`, `layouts[]` |
| `audit_database` | Busca **referencias rotas** (Perform Script / Go to Layout colgados, relaciones/layouts a TOs borradas, campos fantasma) | `xml_path` |
| `who_calls` | Qué dispara un script (Perform Script, triggers, botones) | `xml_path`, `script` |
| `who_uses_field` | Dónde se usa un campo (layouts, claves de relación, Set Field, menciones en cálculos) | `xml_path`, `field` |
| `list_steps` | Catálogo de tipos de step soportados | — |

Cada tool devuelve un bloque de texto con el JSON de la respuesta del motor
(`status`, `data`, `errors`, …) e `isError` cuando el motor reporta error.

## Ejemplo de uso

Una vez configurado, en el chat:

> *"Inspeccioná `~/exports/ventas.xml`, después sliceá el layout
> 'Facturación' y decime si algún Set Field apunta a un campo que no existe."*

La IA, sola, llama `inspect_database` → `slice_inspect` → razona sobre los
`.fmscript` y el esquema, y te devuelve el diagnóstico. Ese es el caso estrella
de la visión (encontrar bugs cruzando scripts + esquema), ahora accesible desde
cualquier cliente IA.

## Probarlo a mano (sin cliente)

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | fm-bridge mcp
```

Deberías ver el `initialize` con `serverInfo` y la lista de tools.
