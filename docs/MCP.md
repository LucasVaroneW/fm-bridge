# fm-bridge MCP server — la puerta IA

`fm-bridge mcp` levanta un servidor **MCP (Model Context Protocol)** por stdio.
Es la **puerta IA**: cualquier cliente MCP (Claude Desktop, Cursor, Antigravity,
Zed…) maneja el **mismo motor** que usa el humano, porque cada tool reenvía a un
comando del binario — no se duplica lógica.

No necesitás Node ni nada extra: es un subcomando del mismo binario que ya viene
empaquetado en el `.vsix` (o instalado en `~/.cargo/bin`).

## ¿El plugin de VS Code ya activa el MCP? No — pero te deja a un paso

Conviene tenerlo claro porque confunde a todo el mundo:

- Instalar el **`.vsix`** te da la **puerta humana** (editar `.fmscript` en VS
  Code) **y el binario en disco** — sin Rust, sin nada extra.
- Pero el **`.vsix` NO configura el MCP** en tu cliente de IA. Ese cliente
  (OpenCode, Claude Desktop, Cursor…) es **otra app, con su propio archivo de
  config en otra carpeta**; la extensión no puede tocarlo. Hay que apuntarlo al
  binario **una vez**.

Las dos puertas son el **mismo motor**, activadas por separado:

```
                 ┌─ .vsix en VS Code ──────────▶ fm-bridge json   (puerta HUMANA)
 fm-bridge.exe ──┤
   (un motor)    └─ config MCP en el cliente IA ▶ fm-bridge mcp    (puerta IA)
```

### La forma fácil (recomendada): desde la extensión

Si tenés la extensión instalada, no escribas rutas a mano:

1. Command Palette → **fm-bridge: Set up MCP for an AI agent**.
2. Elegí tu cliente (OpenCode / Claude Desktop / Cursor).
3. Elegí **Apply to <cliente>'s config**: la extensión **ubica el archivo de
   config de ese cliente para tu SO y escribe/mergea la entrada MCP sola** (con
   la ruta real del binario ya puesta, y un backup `.bak` del original). O elegí
   **Copy to clipboard instead** para el camino manual.
4. Reiniciá el cliente. Listo.

> ¿Ya tenés un agente que puede editar archivos (este mismo, OpenCode, Claude
> Code)? Salteate el comando y pedíselo: *"configurá el MCP de fm-bridge en
> OpenCode"* — te edita el archivo. El comando es el fallback universal para
> cuando todavía no tenés un agente con acceso a archivos.

### A mano (lo mismo, si no usás la extensión)

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

### OpenCode

Otro formato: clave `mcp`, `type: "local"` y el comando como **array**.
En `~/.config/opencode/opencode.jsonc` (global) o un `opencode.json` en la raíz
del proyecto:

```jsonc
{
  "mcp": {
    "fm-bridge": {
      "type": "local",
      "command": ["C:\\Users\\TU_USUARIO\\.cargo\\bin\\fm-bridge.exe", "mcp"],
      "enabled": true
    }
  }
}
```

OpenCode ya es un agente con acceso a archivos, así que ahí también te sirven
`inspect_database` / `slice_inspect` (puede leer las carpetas que generan), no
solo las tools inline.

### Cursor / Antigravity / otros

Mismo patrón que Claude Desktop: en su archivo de MCP servers, comando = ruta al
binario, args = `["mcp"]`.

## Tools que expone

| Tool | Qué hace | Args |
|---|---|---|
| `read_clipboard_script` | Lee el clipboard de FM → `.fmscript` | — |
| `validate_script` | Linter: todos los errores de formato/estructura | `script_text` |
| `script_to_json` | Árbol estructurado del script (steps, calcs, campos) | `script_text` |
| `describe_database` | **Inline**: conteos + nombres de tablas/scripts/layouts/CFs/externos. Primera llamada para orientarse. No escribe a disco | `xml_path` |
| `get_table` | **Inline**: campos de una tabla (tipo, cálculo, indexación, global, stored). No escribe a disco | `xml_path`, `table` |
| `get_script` | **Inline**: el `.fmscript` de un script por nombre o `#id`. No escribe a disco | `xml_path`, `script` |
| `get_layout` | **Inline**: estructura de un layout (objetos, campos, portales, tooltips, **URL de web viewers**, triggers) por nombre o `#id`. No escribe a disco | `xml_path`, `layout` |
| `inspect_database` | Parsea un export `FMSaveAsXML` → **directorio navegable en disco** + conteos | `xml_path`, `output_dir?` |
| `slice_inspect` | Subconjunto enfocado por layout (cierre transitivo) | `output_dir`, `slice_dir`, `layouts[]` |
| `audit_database` | Busca **referencias rotas** (Perform Script / Go to Layout colgados, relaciones/layouts a TOs borradas, campos fantasma) | `xml_path` |
| `who_calls` | Qué dispara un script (Perform Script, triggers, botones) | `xml_path`, `script` |
| `who_uses_field` | Dónde se usa un campo (layouts, claves de relación, Set Field, menciones en cálculos) | `xml_path`, `field` |
| `list_steps` | Catálogo de tipos de step soportados | — |

Cada tool devuelve un bloque de texto con el JSON de la respuesta del motor
(`status`, `data`, `errors`, …) e `isError` cuando el motor reporta error.

> **¿Cliente MCP sin acceso a archivos?** (p. ej. Claude Desktop "pelado", sin un
> filesystem-MCP). Entonces `inspect_database` / `slice_inspect` escriben a disco
> pero no podés volver a leer esa carpeta. Usá las tools **inline**
> (`describe_database` → `get_table` / `get_script`, más `audit_database`,
> `who_calls`, `who_uses_field`): devuelven todo en la respuesta, sin tocar disco.
> Con un solo XML eso alcanza para orientarse y razonar.

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
