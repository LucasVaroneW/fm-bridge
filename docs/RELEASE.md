# Release flow

Lo que hay que correr para buildear y empaquetar una versión nueva.

## 0. Setup (una sola vez)

```powershell
# PATH de MSYS2 mingw64 (no ucrt64):
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"

# Dependencias del plugin:
cd editors/vscode
npm install
cd ../..
```

## 1. Rust binary

```powershell
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test               # que pasen los tests
cargo build --release
cargo install --path .   # instala en ~/.cargo/bin/fm-bridge.exe
```

## 2. VS Code extension

```powershell
cd editors/vscode
npm run typecheck         # verifica tipos TypeScript
npm run build             # compila TS → dist/
```

## 3. Bundle native binary

```powershell
npm run bundle:native     # cargo build + copia binario a bin/<platform>-<arch>/
```

## 4. Package .vsix

```powershell
npm run package           # empaqueta todo en fm-bridge-X.Y.Z.vsix
```

El `.vsix` queda en `editors/vscode/` y contiene:
- La extensión TypeScript compilada
- El binario Rust compilado para la plataforma actual
- Syntax highlighting, snippets, configuración de lenguaje

## 5. Bump version

Antes de release, actualizar la versión en:
- `Cargo.toml` → `version = "X.Y.Z"`
- `editors/vscode/package.json` → `"version": "X.Y.Z"`

## 6. Tag & push

```powershell
git tag v0.1.8
git push --tags
```

El CI de GitHub (`.github/workflows/`) build ea un `.vsix` multiplataforma
(macOS arm64/x64, Windows, Linux) al pushear un tag `v*`.

## 7. Local quick-fix (sin release completa)

Si solo querés probar cambios en tu máquina sin empaquetar, copiá los archivos
directamente a la extensión instalada:

```powershell
$extDir = "$env:USERPROFILE\.vscode\extensions\lucasvarone.fm-bridge-*"
Copy-Item editors/vscode/dist/* $extDir/dist/ -Force
Copy-Item editors/vscode/package.json $extDir/ -Force
Copy-Item ~/.cargo/bin/fm-bridge.exe $extDir/bin/win32-x64/ -Force
```

Y `Developer: Reload Window` en VS Code.
