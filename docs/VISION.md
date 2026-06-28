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

## Estado actual (2026-06-28)

- ✅ **Vía humana (MVP):** extensión VS Code — highlighting, snippets, read/write
  clipboard, diagnostics (multi-error + estructura de bloques + Quick Fix),
  autocomplete desde el catálogo del binario. (#17)
- ✅ **Cero instalación:** binario empaquetado en el `.vsix` + CI multiplataforma.
- ✅ **Motor de scripts:** codec XML↔texto, linter, comando `parse`/`lint`,
  catálogo `steps`, `decode-xml`/`encode-text`.
- ✅ **Opaco por defecto:** steps no reconocidos round-trippean byte-a-byte. (#2)
- ❌ **Vía IA:** todavía no hay MCP server ni salida JSON estructurada del script.
- ❌ **Esquema:** el motor solo entiende scripts (`<Step>`), no tablas/campos/DDR.

## Roadmap por fases

### Fase 1 — Desbloquear la IA (fácil, alta palanca)
- **#3 — JSON estructurado del script.** `FmScript`/`ScriptStep` ya son
  `Serialize`; falta un comando que emita el árbol en JSON (no solo texto).
- **MCP server mínimo** envolviendo lo que ya existe: `read`, `parse/lint`,
  `steps`, `decode-xml`. Con esto una IA ya analiza scripts en lote.

### Fase 2 — El salto grande: esquema (#6 inspect/slice)
- Segunda familia de parsers: definiciones de tabla/campo, layouts, DDR.
- Empezar con un **spike**: copiar una tabla en FM (o exportar DDR), `fm-bridge
  debug`, y mapear el formato XML.
- Exponer por `--json` y como tool del MCP.

### Fase 3 — El oro: bugs con contexto
- Tools que **cruzan** script + esquema (referencias rotas, campos inexistentes,
  scripts huérfanos, etc.). Acá "encontrar bugs solo con extraer XMLs" se vuelve
  real.

### Fidelidad del core (en paralelo, cuando convenga)
- #4 Show Custom Dialog (input fields), #5 Import/Export con DSL legible.

## Issues relacionados

- Épica: **#14** (roadmap completo).
- Core/parser: #2 ✅, #3, #4, #5.
- Esquema: **#6** (inspect/slice).
