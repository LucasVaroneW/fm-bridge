# Evoluciones detectadas durante el uso

> Lista corta y honesta de ideas que aparecieron mientras usábamos la
> herramienta en casos reales. **No implementadas todavía**. Cada item con
> contexto suficiente para retomarlo más adelante.

## Slice navegable, no maximalista

**Problema observado**: el closure transitivo via `Perform Script` ya genera
slices manejables (~30-60 scripts), pero los scripts dentro del closure
contienen `Go to Layout [X]` o `New Window [Y]` hacia layouts que NO están
en el slice. La IA los menciona y queda incompleta.

**No queremos**: expandir el closure a esos layouts. El slice se haría tan
grande como el XML.

**Sí queremos**:
- En `slice_summary.md` listar "referenced but not included" con desde dónde
  se mencionan.
- Comando `fm-bridge expand <slice-dir> <name>` para que la IA pida
  ampliaciones on-demand.

## Lookups inversos para auditoría de bugs

Casos de uso reales que la IA podría querer:
- *"¿Quién llama a `Gen_Borrar_Verificar`?"*
- *"¿Quién dispara este script vía ScriptTrigger?"*
- *"¿Qué scripts/layouts tocan el campo `Pro_Codigo`?"*
- *"¿Qué scripts abren el layout `Eti_Lis_Warning`?"*

Propuesta: `fm-bridge find <output-dir> --who-calls/--who-triggers/--who-uses-to/
--who-uses-field/--who-opens-layout`. Resultado: lista corta de IDs+nombres.

## Workspace de múltiples XMLs

Casos como Dédalo (FM17) + by_xx (varios FMP12) requieren inspeccionar
múltiples XMLs juntos para resolver TOs externas a tablas/campos reales.

Propuesta: `fm-bridge inspect-workspace <carpeta-xmls> <output>` que:
- Corre `inspect` sobre cada XML.
- Une los `external_sources` en un grafo único.
- En las TOs externas, resuelve el schema del archivo correspondiente.
- Una sola `analysis/` global con dependencias cross-file reales.

## Auto-enter + Validation extraction (alta prioridad detectada en `fm-batch-import`)

Hoy el `tables/<Table>.json` reporta `field_type` y `data_type` pero **no**:
- `<AutoEnter type="ConstantData|Calculated|CreationTimestamp|ModificationTimestamp|CreationAccountName|ModificationAccountName|SerialNumber">`
- El `<ConstantData>` o `<Calculation>` interno
- `<Validation type=... notEmpty unique existing allowOverride>`

Para el proyecto `fm-batch-import` (Java pre-calcula auto-enters para usar
`Import Records [doAutoEntry=False]`) esto es CRÍTICO. Hoy lo sacamos a mano
con grep al XML. Propuesta: agregar al parser de `<Field>` la captura de
`<AutoEnter>` y `<Validation>` como sub-objetos. ~80 líneas.

Estructura propuesta para el JSON:
```json
{
  "id": 4, "name": "Ofe_PK", "field_type": "Normal", "data_type": "Text",
  "auto_enter": { "type": "Calculated", "expression": "GetAsString(Get(UUID))", "overwrite_existing": true },
  "validation": { "type": "Always", "not_empty": true, "unique": true, "allow_override": false }
}
```

## Cosas menores

- Algunos `Go to Layout` salen sin `[name #id]` cuando el target está roto
  en FM (`<Layout Missing>`). Hoy aparecen vacíos. Idea: emitir como
  `Go to Layout [<MISSING>]` con TODO.
- Portal con `SortSpecification` puede contaminar el `field_table_occurrence`
  del portal con la TO del sort. Bug menor, no rompe la migración pero
  contamina el campo. Revisar el orden de captura.
- Value Lists (FM enum dropdowns) no se extraen. Útil para mappear a
  `<select>` en frontend.

## Optimizaciones de performance / huellas

- Tooltips/CDATA extraen con `[start..pos_before]` slice. Para XMLs muy
  grandes con tooltips largos eso es fine. No vimos problemas.
- El mermaid global del inspect tiene 2800 líneas. El del slice queda lindo
  (~300 líneas). Considerar también un mermaid "TO graph clusterizado por
  data source" para overview.

## Cosas que sí decidimos NO hacer (anti-roadmap)

- **No** extraer CSS/colores/fonts de layout objects. Objetivo: completitud
  funcional, no fidelidad visual.
- **No** parsear HTML/JS de `<Web Viewer>` objects.
- **No** capturar `LocalCSS` ni temas. Misma razón.
