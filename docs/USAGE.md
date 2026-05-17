# fm-bridge — Guía de uso

CLI en Rust que mueve scripts de FileMaker entre el clipboard y archivos `.fmscript`
de texto plano, para poder editarlos en VSCode (o cualquier editor) en lugar del
Script Workspace de FM.

---

## 1. Qué hace exactamente

FileMaker guarda los scripts copiados con `Cmd+C` en un formato propietario
binario en el clipboard (no es texto plano: tiene un header de 4 bytes y XML
adentro). `fm-bridge` sabe leer ese formato, convertirlo a texto legible, y
hacer el camino inverso.

**Flujo típico:**

```
FM (Cmd+C) → fm-bridge read → script.fmscript → editás en VSCode
                                      ↓
FM (Cmd+V) ← fm-bridge write ← script.fmscript
```

---

## 2. Instalación en una máquina nueva

### 2.1 Instalar Rust

`fm-bridge` está hecho en Rust, así que necesitás el compilador.

**Windows:**

1. Descargar el instalador desde https://rustup.rs
2. Ejecutarlo. Aceptar todas las opciones por defecto (1 → Enter).
3. Cerrar y volver a abrir cualquier terminal (cmd, PowerShell, Git Bash).
4. Verificar: `cargo --version` debe imprimir algo como `cargo 1.8x.x`.

**Mac:**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Aceptar default, después cerrar y abrir terminal
```

### 2.2 Clonar el repo

```bash
git clone https://github.com/LucasVaroneW/fm-bridge.git
cd fm-bridge
```

### 2.3 Compilar e instalar

```bash
cargo install --path .
```

Esto compila en modo release y deja el binario `fm-bridge` en:

- Windows: `C:\Users\<vos>\.cargo\bin\fm-bridge.exe`
- Mac: `~/.cargo/bin/fm-bridge`

Esa carpeta ya está en tu PATH por el instalador de Rust, así que desde
**cualquier directorio** podés escribir `fm-bridge` y funciona.

Verificar:

```bash
fm-bridge --help    # debería listar los comandos
```

Si dice "command not found", reabrí la terminal.

---

## 3. Comandos

### `fm-bridge read`

Lee el clipboard (asumiendo que copiaste algo de FM) y lo imprime como texto en
la terminal. Para guardarlo en archivo:

```bash
fm-bridge read > miscript.fmscript
```

### `fm-bridge write <archivo>`

Lee un archivo de texto `.fmscript` y lo escribe en el clipboard en formato FM.
Después abrís el Script Workspace de FM y hacés `Cmd+V` / `Ctrl+V`.

```bash
fm-bridge write miscript.fmscript
```

### `fm-bridge dump-ids`

Lee el clipboard y te imprime cada step como `id<TAB>nombre`. Se usa para
descubrir el ID numérico de un tipo de step nuevo que aún no esté en
`steps.toml`.

```bash
fm-bridge dump-ids
```

### `fm-bridge debug`

Vuelca a la carpeta actual:

- `debug_raw.xml` — el XML tal cual lo emite FileMaker.
- `debug_built.xml` — el XML que `fm-bridge` regeneraría a partir de eso.

Útil cuando un step se rompe en el round-trip: comparás ambos y ves qué tags
nuestro encoder está perdiendo.

### `fm-bridge passthrough`

Lee el clipboard y lo escribe de vuelta sin modificarlo. Sirve para confirmar
que el problema de transporte (clipboard) está OK y que cualquier bug está en
nuestro parser/encoder.

### `fm-bridge json`

Lee un JSON por stdin y responde un JSON por stdout. Es el modo que va a usar
la futura extensión de VSCode. Formato:

```json
// stdin
{"command": "read"}
{"command": "write", "script_text": "Set Variable [$x = 1]"}
{"command": "version"}

// stdout
{"status": "ok", "script_text": "..."}
{"status": "error", "error": "..."}
```

---

## 4. Workflow real

### Editar un script existente de FM

1. En FM: abrir el Script Workspace, abrir el script, seleccionar todo
   (`Cmd+A` / `Ctrl+A`), copiar (`Cmd+C` / `Ctrl+C`).
2. Terminal:
   ```bash
   fm-bridge read > miscript.fmscript
   ```
3. Abrir `miscript.fmscript` en VSCode, editar.
4. Mandar de vuelta:
   ```bash
   fm-bridge write miscript.fmscript
   ```
5. En FM: pegar (`Cmd+V` / `Ctrl+V`).

### Crear un script desde 0

1. Crear `nuevo.fmscript` en VSCode con la sintaxis de fm-bridge (ver
   ejemplos en [`/test_script.fmscript`](../test_script.fmscript)).
2. `fm-bridge write nuevo.fmscript`
3. En FM: nuevo script vacío, pegar.

---

## 5. Sintaxis del formato .fmscript

Un step por línea. Bloque cerrado con `End If` / `End Loop` se autoindenta.
Comentarios empiezan con `#`. Steps deshabilitados con `// ` adelante.

```
# Esto es un comentario
Set Variable [$contador = 0]
Loop
  Set Variable [$contador = $contador + 1]
  Exit Loop If [$contador >= 10]
End Loop
If [$contador = 10]
  Show All Records
  Perform Script [Get(ScriptName)]
End If
// Set Field [esta línea está deshabilitada]
```

Cálculos multilínea: poné el `[` al final de una línea y `]` solo en la última:

```
Set Variable [$resultado = Let([
    a = 1;
    b = 2
  ];
  a + b
)]
```

---

## 6. Troubleshooting

### "No FM data in clipboard"

No copiaste un step de FM antes de hacer `read`. Volvé a FM, copiá, y reintentá.

### "Step 'X' has no FileMaker ID in steps.toml"

El step que querés escribir no está en el catálogo todavía. Pasos:

1. Crear ese step en FM, copiarlo.
2. `fm-bridge dump-ids` → te imprime el id.
3. Editar `steps.toml` y agregar la entrada con su id.
4. Recompilar: `cargo install --path .`

### Pego en FM y aparece un step distinto al que escribí

El ID que tiene esa entrada en `steps.toml` está mal (apunta a otro step de FM).
Repetí el `dump-ids` para esa entrada y corregí el id en el toml.

### Pego en FM y aparece el step correcto pero con los parámetros vacíos

La `shape` de ese step en `steps.toml` es `plain` pero el step en realidad lleva
parámetros (cálculo, target, etc.). Hay que cambiarle la shape. Las shapes
posibles están documentadas arriba del propio `steps.toml`. Para identificar
cuál corresponde, mirá `debug_raw.xml` después de un `fm-bridge debug` con ese
step en el clipboard.

### "Cannot open clipboard after 30 attempts" (Windows)

Otro proceso está bloqueando el clipboard (típicamente clipboard managers tipo
Ditto, ClipClip, etc.). Cerralo temporalmente y reintentá.

### El binario `fm-bridge` no se encuentra después de instalarlo

Cerrá y reabrí la terminal. Si seguís en la misma sesión que usaste para
instalar Rust, el PATH no está actualizado.

---

## 7. Estructura del proyecto

```
fm-bridge/
├── Cargo.toml              ← dependencias y metadata
├── steps.toml              ← catálogo de tipos de step (id, nombre, shape)
├── docs/USAGE.md           ← este archivo
├── test_script.fmscript    ← script de ejemplo para probar
├── ids.txt                 ← dump de referencia de IDs descubiertos
└── src/
    ├── main.rs             ← entrypoint, dispatch de comandos
    ├── clipboard.rs        ← I/O al clipboard del SO (Win32 + macOS)
    ├── xmss.rs             ← codec del formato XML de FM
    ├── text_format.rs      ← codec del formato .fmscript de texto
    └── steps.rs            ← carga steps.toml, helpers de búsqueda
```

---

## 8. Cómo modificar y republicar

Después de editar cualquier archivo en `src/` o `steps.toml`:

```bash
cargo install --path .   # recompila y reemplaza el binario instalado
```

Para confirmar que compila sin instalarlo:

```bash
cargo build --release
```

Para subir cambios:

```bash
git add -A
git commit -m "descripción del cambio"
git push
```
