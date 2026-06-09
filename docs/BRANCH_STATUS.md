# Rama `feature/xml-inspector` — estado actual

> Documento vivo. Resume qué hace la rama, hacia dónde va y qué queda.
> No mezclar con `USAGE.md` (manual de comandos) ni con `IA-PROMPT.md`
> (instrucciones para LLMs).

## En una línea

Darle a una IA "ojos" dentro de FileMaker sin abrir FM: parsear un export
`FMSaveAsXML`, exponer scripts/layouts/relaciones en archivos editables, y
construir slices enfocados para tareas de migración o auditoría.

## Estado actual (rama lista para mergear cuando se decida)

- `xmss.rs` (clipboard) **intacto**. 26/26 tests pasan. El flujo `read`/`write`
  no se tocó.
- Nuevo comando `fm-bridge inspect <archivo.xml> <output-dir>`. Streaming,
  soporta XMLs UTF-16 de 100MB+. Extrae:
  - Scripts (calidad clipboard, con states/cálculos/targets correctos)
  - Layouts completos: TO base, objetos recursivos (portales con `children[]`),
    object script_triggers (OnObjectExit, OnObjectModify, …),
    layout_triggers (OnLayoutEnter, OnRecordCommit, …), tooltips
  - 1 TableOccurrence → `(archivo externo, tabla base)`
  - Relationships con join predicates completos
  - Custom Functions con cuerpo
  - ExternalDataSources
  - `analysis/analysis.json` con grafo de llamadas, scripts no usados,
    botones que disparan scripts, dependencias por archivo externo
  - `relationships.mmd` (Mermaid ER diagram)
- Nuevo comando `fm-bridge slice <output> <slice> <layout-name>…`. Reduce el
  export entero a ~30 archivos sobre los layouts pedidos. Closure transitivo de
  scripts via call graph + TOs + relations + custom functions usadas en los
  cálculos.

Métricas reales (XML logística, 144 MB):
- 296 scripts exportados, 451 layouts, 833 TOs, 675 relations, 16 CFs
- Slice `Sto_Dat_Gen + Sto_Dat_Lis`: 634 KB total (227× reducción) con todo
  el contexto funcional.

## Hacia dónde vamos

Foco: que una IA pueda **migrar módulos completos** a Angular o **auditar
flujos por bugs** sin que se le escape ninguna funcionalidad oculta.

Próximos pasos (no implementados todavía):

1. **Slice navegable por la IA**, no maximalista. Cuando el closure encuentra
   un `Go to Layout [X]` o `Perform Script [Y]` que NO está en el slice,
   listarlo en `slice_summary.md` como "referenced but not included" para
   que la IA pueda decir al usuario *"pasame el slice de X"*.
2. **`fm-bridge expand <slice-dir> <name>`** — agregar on-demand un layout o
   script al slice sin rehacer todo.
3. **`fm-bridge find`** con lookups inversos para que la IA cace bugs:
   - `--who-calls <script>`
   - `--who-triggers <script>`
   - `--who-uses-to <to-name>`
   - `--who-uses-field <to::field>`
   - `--who-opens-layout <layout-name>`
4. **Workspace de inspects** — comando para tomar N XMLs (un archivo principal
   + sus archivos externos) y producir un output unificado donde las TOs se
   resuelven a tablas reales con sus campos.

Ver `EVOLUTIONS.md` para ideas de evolución detectadas durante el uso.

## Lo que la rama explícitamente NO hace

- No extrae CSS / fuentes / colores de objetos (es decisión: el objetivo es
  completitud funcional, no fidelidad visual).
- No parsea el contenido HTML/JS de `<Web Viewer>` objects.
- No resuelve campos de tablas que viven en archivos externos. Solo el
  nombre del archivo y la tabla, no su schema.
- No extrae value lists ni validations de external tables.
