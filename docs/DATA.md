# Datos en vivo (ODBC)

fm-bridge lee **estructura** de los exports `FMSaveAsXML`. Con esta capa lee
además **datos reales** de una base hospedada, por ODBC — para que una IA pueda
cruzar las dos mitades: ver que un campo es un cálculo no almacenado *y* mirar
qué valores tiene de verdad.

Todo lo que hay acá es **solo lectura**.

## Cómo está armado

```
fm-bridge            (el motor, sin dependencias nativas)
   └─ lanza ─▶  fm-bridge-odbc     (un proceso por consulta)
                     └─ carga ─▶  el driver ODBC de FileMaker
                                   (instalado por vos, no incluido acá)
```

Tres decisiones que conviene entender:

- **Un proceso por consulta.** No hay pool ni demonio. Cuando el proceso
  termina, el sistema operativo cierra el socket: una conexión huérfana en el
  servidor no es algo que haya que recordar evitar, directamente no puede
  pasar. Reconectar cuesta unos cientos de milisegundos.
- **El padre mata, no pide permiso.** Un plazo de reloj de pared
  (`kill_timeout_s`) respalda al driver. Un driver ya colgado es justo el que
  va a ignorar su propio timeout; matar el proceso siempre funciona.
- **Una consulta por vez.** Un modelo puede lanzar diez llamadas en paralelo;
  serializarlas es lo que evita que eso sean diez conexiones en producción.

fm-bridge **no distribuye ningún driver de Claris**. Habla ODBC genérico contra
el administrador de controladores del sistema (`odbc32.dll` en Windows, que es
de Microsoft), y ese carga el driver que instalaste vos.

## Qué hay que instalar

1. **El driver ODBC de FileMaker/Claris.** Descarga gratuita de Claris; muchas
   veces ya viene con FileMaker Pro o FileMaker Server.
2. **El sidecar** `fm-bridge-odbc`, al lado del binario `fm-bridge`.

> **Arquitectura — la trampa número uno.** El driver se carga *dentro* del
> proceso, así que las arquitecturas tienen que coincidir. FileMaker Pro suele
> registrar **solo el driver de 32 bits**, y un `fm-bridge-odbc.exe` de 64 bits
> no puede cargarlo: falla con `IM002 — no se encuentra el nombre del origen de
> datos`, que no dice una palabra sobre bits.
>
> Por eso el motor busca **`fm-bridge-odbc-x86`** primero y cae a
> `fm-bridge-odbc` si hace falta. Compilá el de 32 bits y nombralo así, y el
> usuario nunca se entera de que la palabra "bitness" existe.

En el servidor, por cada archivo `.fmp12`:

- **Archivo ▸ Compartir ▸ Habilitar ODBC/JDBC**.
- Una cuenta cuyo conjunto de privilegios tenga el privilegio extendido
  **`fmxdbc`** (*Acceso vía ODBC/JDBC*), en **Archivo ▸ Gestionar ▸ Seguridad**.

Ese conjunto de privilegios es la frontera de seguridad real: fm-bridge no
puede concederse nada. **Usá una cuenta de solo lectura.**

## `.fm-bridge.toml`

Va en la raíz del proyecto de VS Code. Se busca hacia arriba desde el directorio
actual, así que funciona desde cualquier subcarpeta.

```toml
[[server]]
name = "produccion"          # etiqueta libre
host = "10.0.0.5"
user = "solo_lectura"
# driver = "FileMaker ODBC"  # opcional, si lo renombraste

[[database]]
name   = "Stock"             # nombre lógico que usan las tools
server = "produccion"
odbc   = "By_20_Stock"       # el archivo tal como lo expone ODBC
xml    = "fm/By_20_Stock.xml"  # opcional: el export de ESTA misma base

[limits]
max_rows          = 500
connect_timeout_s = 15
kill_timeout_s    = 45
max_cell_chars    = 500    # tope por celda
max_total_chars   = 20000  # tope del resultado entero
```

`max_total_chars` importa más de lo que parece: una table occurrence de
FileMaker tiene fácil 60 o 95 columnas, así que `max_rows × columnas ×
max_cell_chars` puede ser enorme aunque los otros dos topes se respeten. Este es
el que de verdad evita que un `SELECT *` te vacíe la ventana de contexto.
Cuando se alcanza cualquier tope, la respuesta trae `truncated: true` — **no
saques conclusiones de totales sin mirar ese campo**.

El campo **`xml` es la pieza que fusiona las dos mitades**: le dice al motor qué
export describe qué base viva. Con eso la IA puede mirar el esquema antes de
gastar una consulta.

Este archivo está pensado para commitearse. **Las contraseñas no van acá** — si
ponés una clave `password`, la carga falla a propósito.

## Contraseñas

Por orden de precedencia:

1. Variable de entorno `FMBRIDGE_PASSWORD_<SERVIDOR>` (mayúsculas, lo que no sea
   alfanumérico pasa a `_`). Ej.: `FMBRIDGE_PASSWORD_PRODUCCION`.
2. `fm-bridge data login <servidor>`, que la guarda en el directorio de
   configuración del usuario (`%APPDATA%\fm-bridge\credentials.toml` en
   Windows), fuera de cualquier proyecto.

> El archivo de credenciales es texto plano, legible por tu usuario. Es una
> mejora frente a tener la clave en el repo, no una bóveda: usá una cuenta de
> solo lectura. (Guardarla en el llavero del sistema es el siguiente paso.)

## CLI

```bash
fm-bridge data list                     # bases configuradas
fm-bridge data doctor [base]            # diagnóstico con el arreglo incluido
fm-bridge data login <servidor>
fm-bridge data query <base> <TO> [filtro]
fm-bridge data count <base> <TO> [filtro]
fm-bridge data sql   <base> "SELECT …"
```

`data doctor` es lo primero que hay que correr cuando algo falla por un motivo
que no es el SQL. Verifica sidecar, credenciales, driver, host, cuenta y
compartición, y devuelve cada chequeo con qué hacer.

## Tools MCP

| Tool | Para qué |
|---|---|
| `list_databases` | Qué bases hay y qué XML describe a cada una. No conecta. |
| `query_table` | Leer filas **sin escribir SQL**: el motor compone y entrecomilla. |
| `count_rows` | `COUNT(*)` con filtro opcional — medir antes de traer. |
| `query_sql` | Un `SELECT` libre, para joins y agregados. |
| `data_doctor` | Diagnóstico con arreglos en lenguaje humano. |

## Por qué las lecturas son libres y las escrituras no existen (todavía)

Una lectura no tiene nada que deshacer: una consulta mal hecha devuelve basura y
se prueba otra. Poner aprobaciones ahí sería costo puro y mataría el flujo de
investigación. Así que **los `SELECT` no tienen fricción**.

Los límites que sí existen —timeout, tope de filas, una consulta por vez— no son
filtros de seguridad: existen para que la investigación **no se corte** (un
`SELECT` desbocado vacía la ventana de contexto del modelo y castiga al
servidor).

Escribir sí tiene algo que deshacer, así que el validador acepta **una sola
sentencia y solo si es un `SELECT`** — lista blanca sobre un parseo, no búsqueda
de palabras prohibidas. Un `DELETE` escondido en un literal, en un comentario
`/* */` o detrás de un `;` no pasa, y lo que no se reconoce falla **cerrado**.
Cuando haya escrituras, van a ser propuestas con conteo de filas afectadas y
`rollback.sql`, aprobadas por una persona.

## Trampas de FileMaker que siguen ahí

Son del motor de FileMaker, no de esta capa:

- En el `FROM` va una **table occurrence**, no una tabla base.
- Límite de filas: `FETCH FIRST n ROWS ONLY`, nunca `LIMIT`.
- Identificadores con `_`, espacios o acentos, entre comillas dobles.
- Los **campos calculados no almacenados** son lentísimos por ODBC.
- Un cálculo que hace `ExecuteSQL` **cruzado a otro archivo** devuelve `?` en una
  sesión ODBC. Consultá la tabla de origen en su propio archivo.
- Claves numéricas guardadas como texto: compará con `'537'`, no `537`.

## Compilar y empaquetar el sidecar

Normalmente no hace falta hacer nada a mano:

- **`npm run package:bundled`** (en `editors/vscode/`) compila el motor y el
  sidecar y los mete en `bin/<plataforma>-<arch>/`. Si el sidecar no compila,
  avisa y sigue: el `.vsix` sale igual, sin datos vivos.
- **El workflow `Package extension`** los compila para todas las plataformas,
  incluidas las arquitecturas alternativas (32 bits en Windows, x86_64 en Mac
  con Apple Silicon).

A mano:

```bash
cargo build --release -p fm-bridge-odbc                                 # arquitectura actual
cargo build --release -p fm-bridge-odbc --target i686-pc-windows-msvc   # Windows 32 bits
```

Copiá el resultado junto a `fm-bridge`; el alternativo, renombrado a
`fm-bridge-odbc-x86[.exe]`.

Un `cargo build` normal **no** compila el sidecar (`default-members = ["."]`):
es el único componente que linkea un administrador de controladores nativo, y se
mantiene aparte para que el motor siga compilando en cualquier lado sin
dependencias del sistema.

**Qué gestor ODBC se linkea** está fijado en el `Cargo.toml` del sidecar, porque
es una decisión de distribución:

| Plataforma | Gestor | Por qué |
|---|---|---|
| Windows | `odbc32.dll` | Es de Microsoft y viene con el sistema. |
| macOS | **iODBC** | Viene con macOS. Linkear el unixODBC de Homebrew daría un binario que anda en la máquina que compila y falla en la del usuario. |
| Linux | unixODBC | Requiere `unixodbc-dev` al compilar y unixODBC instalado al correr. |
