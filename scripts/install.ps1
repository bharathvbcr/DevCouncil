param(
    [switch]$Editable
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    Write-Error "uv is required. Install it from https://docs.astral.sh/uv/getting-started/installation/ and rerun this script."
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
    if ($Editable) {
        uv pip install -e .
        Write-Host "DevCouncil installed in the current environment. Try: uv run devcouncil --help"
    } else {
        uv tool install --force .
        Write-Host "DevCouncil installed as a uv tool. Try: devcouncil --help"
    }
} finally {
    Pop-Location
}
