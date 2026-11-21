# Builds the web example into examples/web/pkg (index.html loads it from there).
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    wasm-pack build --release --target web
    if ($LASTEXITCODE -ne 0) { throw "wasm-pack failed" }
} finally {
    Pop-Location
}
