# fm-bridge — Visión y Roadmap

> El norte del proyecto. Si empezás de cero (o una IA arranca un chat nuevo),
> esto es **a dónde queremos llegar**. Las tareas concretas viven en los Issues;
> esto es el _qué_ y el _por qué_.

## La idea en una frase

Una herramienta para trabajar scripts (y, a futuro, esquema) de FileMaker con
**dos puertas de entrada sobre un mismo motor**: una **humana** (editar
`.fmscript` en VS Code) y una **IA** (vía MCP / CLI con JSON). Las dos se usan
combinadas y son intercambiables.

## Principios (no negociables)

1. **Cero instalación para el usuario final.** Un dev de FileMaker instala
   **solo el `.vsix`** — nada de instalar Rust ni hacer malabares. El binario va
   **empaquetado dentro de la extensión** (`bin/<plataforma>/`), y el CI arma un
   `.vsix` **universal** (macOS arm64/x64, Windows, Linux). Doble click y listo.
2. **Un solo motor, clientes finos.** Toda la lógica (parser, linter, codec
   XML↔texto, futuro inspect) vive en el **binario Rust**. La extensión y el
   futuro MCP server son envoltorios finos. Nunca duplicar lógica en un cliente.
3. **Humano e IA comparten los mismos artefactos.** Ambos leen/escriben
   `.fmscript` (texto) y JSON. Por eso son **intercambiables**: si la IA se queda
   sin tokens, un humano sigue donde quedó (y viceversa). El terreno común son
   archivos planos, no el estado interno de nadie.
4. **Lossless / opaco por defecto.** Lo que no entendemos se **preserva tal
   cual** (round-trip byte-a-byte), nunca se descarta. (Ver #2.)

## Las dos vías

```
                    ┌─────────────────────┐
   Humano   ─────▶  │   Extensión VS Code │ ─┐
   (editar)         └─────────────────────┘  │
                                             ├──▶  binario fm-bridge  ──▶  XML de FM
                    ┌─────────────────────┐  │     (parser · linter · codec · inspect)
   IA (MCP) ─────▶  │   MCP server (TODO) │ ─┘
   (analizar)       └─────────────────────┘
```

**Caso de uso estrella:** una IA extrae XMLs de varias bases, los pasa por el
motor (decode + lint + inspect), y encuentra bugs rápido — cruzando scripts con
el esquema (ej.: un `Set Field` que apunta a un campo inexistente).

## Estado actual (2026-06-29)

- ✅ **Vía humana (MVP):** extensión VS Code — highlighting, snippets, read/write
  clipboard, diagnostics (multi-error + estructura de bloques + Quick Fix),
  autocomplete desde el catálogo del binario, comandos `inspect`/`slice`. (#17)
- ✅ **Cero instalación:** binario empaquetado en el `.vsix` + CI multiplataforma.
- ✅ **Motor de scripts:** codec XML↔texto, linter, comando `parse`/`lint`,
  catálogo `steps`, `decode-xml`/`encode-text`.
- ✅ **Opaco por defecto:** steps no reconocidos round-trippean byte-a-byte. (#2)
  Import/Export Records ahora se ven como **DSL indentado y editable** con
  round-trip byte-a-byte. (#5)
- ✅ **Esquema (#6) — el salto grande, hecho:** `inspect` parsea el export
  `FMSaveAsXML` y genera carpeta navegable con **tablas + campos calculados +
  indexación** (index/indexed/global/stored), layouts (objetos recursivos,
  portales, triggers, tooltips), TOs resueltas a archivo externo, relaciones con
  joins, custom functions, y **scripts fieles al `read` byte-a-byte** (incluido
  cross-file Perform Script, Set Field/Replace, comentarios, nombres de variable)
  organizados en **carpetas** como en FM. `slice` arma el subconjunto enfocado
  por cierre transitivo. Todo expuesto también por `--json` para una IA headless.
- ✅ **Vía IA — hecha:** **#3** (árbol del script en JSON via `to_json`) + un
  **MCP server** (`fm-bridge mcp`, JSON-RPC por stdio, sin async ni deps nuevas)
  que expone `read`/`validate`/`script_to_json`/`inspect`/`slice`/`steps` como
  tools. Cualquier cliente MCP (Claude Desktop, Cursor, Antigravity) maneja el
  mismo motor que el humano. Ver [MCP.md](MCP.md).

## Roadmap por fases

### Fase 1 — Desbloquear la IA — ✅ hecho
- ✅ **#3 — JSON estructurado del script** (`to_json`: texto → árbol de steps).
- ✅ **MCP server** (`fm-bridge mcp`) envolviendo `read`/`validate`/
  `script_to_json`/`inspect`/`slice`/`steps`. Puerta IA formal. Ver [MCP.md](MCP.md).

### Fase 2 — El salto grande: esquema (#6 inspect/slice) — ✅ hecho
- ✅ Segunda familia de parsers (`fmsavexml.rs`): tablas/campos (con cálculo e
  indexación), layouts, TOs, relaciones, custom functions, scripts en carpetas.
- ✅ Expuesto por `--json` (`inspect`/`slice`) y por la extensión de VS Code.
- Pendiente menor: campos de tablas **externas** viven en el `inspect` de su
  propio archivo, no dentro del slice (hay que cruzar dos exports).

### Fase 3 — El oro: bugs con contexto
- Tools que **cruzan** script + esquema (referencias rotas, campos inexistentes,
  scripts huérfanos, `who-calls`/`who-uses-field`, etc.). Hoy se hace **a mano**
  con inspect/slice (ej.: el diagnóstico del bug de las SX); falta automatizarlo.

### Fidelidad del core (en paralelo, cuando convenga)
- #4 Show Custom Dialog (input fields). #5 Import/Export con DSL legible ✅.
- Pendiente conocido: indentación de cálculos multilínea en el round-trip.

## Issues relacionados

- Épica: **#14** (roadmap completo).
- Core/parser: #2 ✅, #3, #4, #5.
- Esquema: **#6** (inspect/slice).
