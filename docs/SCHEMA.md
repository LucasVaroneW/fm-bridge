# Esquema como código y salida a otra DB (Fase 4)

> **Estado: en diseño.** No hay código todavía. Este documento fija el alcance,
> los veredictos de viabilidad y —sobre todo— **lo que decidimos no hacer**, para
> que la implementación no se descubra a mitad de camino.
>
> Norte del proyecto: [VISION.md](VISION.md). Datos en vivo: [DATA.md](DATA.md).

## Por qué esto existe

Hoy `inspect` sabe leer el esquema de una base FileMaker. Lo que falta es **hacer
algo con eso**: llevarlo fuera, versionarlo, y devolverlo.

Tres pedidos reales, que son tres productos distintos:

| | Qué es | Dirección | Veredicto |
|---|---|---|---|
| **A. Esquema como código** | `init` / `apply` / `diff` / `fix` dentro de FileMaker | texto → FM | 🟡 mitad hecha, mitad apuesta |
| **B. Muchas tablas juntas** | copiar/pegar N tablas de una | FM ↔ texto | ✅ viable, y obligatorio |
| **C. Salida a otra DB** | migrar una app FM a Postgres/MySQL | FM → SQL | ✅ el más viable |

**C va primero.** No por importancia, por riesgo: solo lee (no puede romper el
archivo de nadie), se apoya en parsers que ya existen, y **define el formato del
modelo que A necesita**. Si A resulta imposible, C sigue siendo un producto
entero.

## Lo que ya está construido

El cálculo de esfuerzo cambia mucho cuando se mira el inventario real. `inspect`
(`src/fmsavexml.rs`) ya parsea:

| Dato | Dónde | Sirve para |
|---|---|---|
| Campos: tipo, cálculo, comentario | `FieldInfo` (:113) | esquema |
| Indexación, global, stored, repeticiones | :123-137 | esquema + índices |
| Validación: notEmpty, unique, calc, mensaje | `FieldValidation` (:154) | constraints |
| Auto-entrada: tipo + cálculo | `FieldAutoEnter` (:176) | defaults / serials |
| Relaciones con **operador real** y multi-predicado | `JoinPredicate` (:225) | claves foráneas |
| TOs → tabla base, internas y externas | `TableOccurrence` (:213) | colapsar el grafo |
| Custom functions con cuerpo | `CustomFunction` (:200) | documentar lógica |
| Lectura de datos vivos por ODBC | `src/data.rs` | migrar filas |

El lector está en un ~80%. Lo que falta es un **traductor** sobre estructuras que
ya existen, no un parser nuevo.

## La decisión de diseño central

**La fuente de verdad es un modelo rico. El SQL neutro es una proyección.**

```
modelo/contabilidad.fmschema     ← en git: tipos + calcs + auto-enter + relaciones
        │
        ├── schema export --sql postgres  → portabilidad a otro motor
        ├── schema apply --clipboard      → pegar en FM, alta fidelidad
        ├── schema diff <export.xml>      → drift → V001__fix.fmschema
        └── schema profile                → tipos decididos con datos reales
```

Es tentador que el archivo versionado sea directamente SQL neutro —es portable y
todo el mundo lo lee—, pero entonces el modelo queda empobrecido al nivel del
canal más pobre: en `CREATE TABLE` no entran ni la auto-entrada, ni los cálculos,
ni los campos de sumario, ni las listas de valores. De rico a neutro se puede
proyectar; al revés no se recupera nada. Y la proyección a Postgres sale **mejor**
desde el modelo rico, porque desde ahí sabemos que un campo era un serial y lo
traducimos a `SERIAL`; desde el DDL de FileMaker ya no se sabe.

## Los tres canales

Copiar/pegar y ODBC no compiten: hacen cosas distintas.

| Canal | Fidelidad | Automatismo | Rol |
|---|---|---|---|
| **Portapapeles** (`XMTB`/`XMFD`) | Alta — calcs, auto-enter, validación | Requiere **un** pegado humano | **Aplicar** el modelo |
| **ODBC DDL** | Baja — solo tipos e índices | Total, la IA sola | Plan B, sin humano delante |
| **`inspect`** (export `FMSaveAsXML`) | Alta, ya funciona | Requiere exportar | **Leer** la realidad → diff |

Como el portapapeles sí lleva auto-entrada y cálculos, **crea esquemas mejores que
ODBC**. ODBC además exige el privilegio extendido `fmxdbc` y una cuenta con
permisos de esquema, o sea lo contrario de la cuenta de solo lectura que
recomienda [DATA.md](DATA.md). El camino principal es el portapapeles.

## C — Salir de FileMaker

Una app FileMaker son tres cosas, y **solo dos se migran**:

```
Esquema  ─── automatizable ~85%
Datos    ─── automatizable ~90%   (ODBC ya lee)
Lógica   ─── automatizable  ~0%   scripts, layouts, cálculos = reescritura
```

Ser explícito con la tercera línea es parte del producto. El mensaje correcto es
*"esto te ahorra las dos semanas de tipear esquema y mover datos; los seis meses
de reescribir la app siguen siendo tuyos"*. Prometer más se paga con el primer
usuario que lo prueba.

### Viabilidad concepto por concepto

| Concepto FM | Destino SQL | Veredicto |
|---|---|---|
| Tabla, campo, tipo | `CREATE TABLE` | ✅ directo |
| Índice | `CREATE INDEX` | ✅ directo |
| `notEmpty` / `unique` | `NOT NULL` / `UNIQUE` | ✅ directo |
| Auto-entrada Serial | `SERIAL` / `IDENTITY` | ✅ directo |
| Auto-entrada UUID / fecha creación | `DEFAULT gen_random_uuid()` / `now()` | ✅ directo |
| Validación por cálculo | `CHECK` | 🟡 solo si el calc es trivial |
| Cálculo almacenado | columna generada | 🟡 depende del calc |
| Cálculo no almacenado | vista | 🟡 depende del calc |
| Relación 1-predicado `=` a campo único | `FOREIGN KEY` | 🟡 heurística |
| Relación multi-predicado o `≠ < > ×` | — | ❌ solo se documenta |
| Campo de sumario | vista agregada | 🟡 |
| Repeticiones (`maxRepetitions`) | tabla hija o N columnas | 🟡 decisión humana |
| Campo global | no es columna | ❌ es estado de app |
| Contenedor | `BYTEA` + extracción de archivos | 🟡 caro, fuera de v1 |
| Listas de valores | tabla de lookup o `CHECK` | ❌ **no las parseamos aún** |
| Auto-entrada `Looked_up` | — | ❌ **fuente sin resolver** (`fmsavexml.rs:174`) |
| Scripts, layouts, portales | — | ❌ es la app, no la DB |

### Las cinco minas

1. **Los tipos de FileMaker son mentira.** Un campo "número" acepta texto; un
   campo fecha importado de un CSV puede tener basura. Postgres rechaza el
   `INSERT` en la fila 40.000 y se van tres días en encontrar cuál. Esto hunde más
   migraciones que ninguna otra cosa. La respuesta no es un traductor más listo,
   es **perfilar los datos reales antes de emitir el DDL**. Ojo: `data_doctor`
   (`src/data.rs:397`) **no** hace esto — es un chequeo de conectividad.
   `schema profile` es una herramienta nueva.
2. **Tablas sin clave primaria.** FileMaker nunca la exigió, y las bases viejas no
   la tienen. Sin PK no hay FK ni migración incremental. Salida: la pseudo-columna
   **`ROWID`** del SQL de FileMaker, como clave sintética estable.
3. **Relaciones ≠ claves foráneas.** El grafo relaciona *occurrences*, no tablas:
   cinco TOs de `CLIENTES` colapsan a una tabla. Se emite `FOREIGN KEY` **solo**
   con predicado único, operador `=` y lado derecho con índice único. Todo lo
   demás se documenta y no se emite: una FK a ojo produce un esquema que no carga
   los datos, que es peor que no emitir nada.
4. **Los cálculos no se traducen.** `Let`, `Case`, `Get(…)`, `GetNthRecord` y sobre
   todo la travesía de relaciones (`CLIENTES::nombre`, que en SQL es una
   subconsulta correlacionada). Traducir eso es un compilador entero **que
   fallaría en silencio**. Se emite la columna con el cálculo FileMaker original al
   lado, como comentario y `TODO`. Un humano lo resuelve en minutos.
5. **Nombres.** FileMaker permite espacios, acentos y símbolos. Hace falta un
   **mapa de nombres explícito y reversible**, emitido como archivo, porque el DDL
   y el volcado de datos tienen que coincidir exactamente.

## B — Copiar varias tablas juntas

**No es comodidad, es corrección.** Si se pega `FACTURAS` sin `LINEAS`, cualquier
cálculo o auto-entrada de `FACTURAS` que referencie `LINEAS::importe` entra roto.
Las tablas de un modelo son un paquete, no items sueltos: el formato es
multi-tabla desde el día uno, aunque la UI empiece copiando una.

Dos riesgos:

- **FileMaker no avisa de colisiones**: si la tabla ya existe crea `CLIENTES 2` en
  silencio. Hace falta un *pre-flight* contra el `inspect` del destino que avise
  **antes** de tocar el portapapeles.
- **Las relaciones no viajan.** Se pegan 10 tablas y el grafo queda vacío. Es
  trabajo manual y el comando tiene que decirlo de frente.

## A — Esquema como código dentro de FileMaker

| Mitad | Estado |
|---|---|
| **Leer** (`diff`, drift, refresh) | ✅ `inspect` ya lo da casi entero |
| **Escribir** (generar `XMTB`) | ⚠️ **apuesta**: formato sin documentar |

`diff` y `fix` salen casi gratis y son la mitad de más valor. Y como el export trae
los **IDs de campo** (`fmsavexml.rs:114`), el diff distingue un *rename* de un
*borrar + crear* — exactamente donde se rompen las herramientas de migración del
mercado.

El generador de `XMTB` es otra cosa. Mitigación, aplicando el principio 4: **no
sintetizar el XML desde cero**; capturar un payload real de FileMaker como
plantilla y generar por clonado y sustitución, preservando verbatim lo que no
modelamos. Menos elegante, mucho menos frágil.

Límite que no desaparece: **el portapapeles solo añade**. Un campo que cambió de
tipo o de cálculo no se arregla pegando; `diff` dice qué tocar, pero aplicarlo es
ODBC o mano humana.

## El libro de cuentas

Principio 5 de [VISION.md](VISION.md), hecho concreto. Cada comando que transforme
esquema o datos emite algo así:

```
CLIENTES: 34 campos leídos → 31 convertidos, 3 NO convertidos
  · foto            contenedor      → requiere extracción de archivos
  · totales[12]     repeticiones    → decidí: tabla hija o 12 columnas
  · saldo           calc no almac.  → cálculo FM emitido como TODO
Relaciones: 18 leídas → 7 emitidas como FK, 11 documentadas (no son FKs)
```

Y la garantía mecánica: cada elemento y atributo del XML de origen o está
modelado, o está en la lista de no convertidos. **Si la cuenta no cuadra, el
comando falla.**

Consecuencia esperada y buscada: esto **va a destapar bugs de `inspect`**. Hoy el
parser se usa para mirar cosas puntuales y un campo mal leído pasa desapercibido;
una migración lo fuerza a procesar cada campo de cada tabla y contrastarlo con
datos reales. Dos gaps ya conocidos antes de empezar: las **listas de valores no se
parsean en absoluto** y la **fuente de los auto-enter `Looked_up` no se resuelve**.

## Lo que NO se hace

- **Traducir el lenguaje de cálculo de FM a SQL.** Compilador entero, falla en
  silencio, corrompe datos.
- **Migrar layouts o scripts.** Es la app, no la base de datos.
- **DDL por ODBC como camino principal.** Queda de plan B.
- **Contenedores en v1.** El reporte dice "estos 3 campos tienen archivos,
  resolvelos aparte".

## Orden de implementación

| Fase | Qué | Depende de |
|---|---|---|
| **4a** | `schema export` — FM → modelo rico → SQL neutro + datos + libro de cuentas | nada nuevo |
| **4b** | `schema profile` — tipos decididos con datos reales por ODBC | 4a |
| **4c** | `schema diff` — modelo vs realidad, renames por ID | 4a |
| **4d** | `schema apply --clipboard` — multi-tabla, la apuesta | verificaciones |

4a, 4b y 4c **no dependen** de que el generador de `XMTB` funcione.

## Antes de escribir código: verificar contra FileMaker real

Base de descarte, media mañana. Definen el 80% del diseño y pueden matar ramas
enteras antes de que cuesten caro.

1. Copiar una tabla con UUID auto-entrado y un cálculo → `fm-bridge read`. **¿Trae
   `<AutoEnter>` y `<Calculation>`?** Define si el portapapeles es realmente el
   canal de alta fidelidad.
2. Copiar **tres tablas juntas** → ¿un snippet con tres `<BaseTable>`?
3. `SELECT ROWID FROM …` por ODBC → ¿funciona? Define la estrategia de PK para
   tablas sin clave.
4. Un `CREATE TABLE` de tres campos por ODBC → ¿existe el plan B? ¿Crea también la
   table occurrence en el grafo?
