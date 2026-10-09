# First-time bootstrap for DevCouncil on Windows.
#
# `devcouncil install` is the installer. This script only gets a host binary
# to run it: it builds the Go host from this checkout into a temporary
# directory and runs `devcouncil install` (or `devcouncil uninstall` with
# -Uninstall), translating these parameters to its flags. Every preset and
# health check belongs to that command; -Help prints them.
#
#   .\scripts\install.ps1                          # host + every analysis binary
#   .\scripts\install.ps1 -Components devmap       # one component, no host
#   .\scripts\install.ps1 -Uninstall -Components dcgrep -Yes
#
# Without Go, devmap alone installs with
#   cargo install --git https://github.com/bharathvbcr/DevCouncil.git --locked devmap-cli
param(
    [string]$Prefix = "",
    [string[]]$Components = @(),
    [switch]$Uninstall,
    [switch]$All,
    [switch]$Help,
    [switch]$HostOnly,
    [switch]$Yes,
    [switch]$List,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command go -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine("go is required: this script builds the DevCouncil host, and the host is the installer.")
    [Console]::Error.WriteLine("Without Go, install devmap alone: cargo install --git https://github.com/bharathvbcr/DevCouncil.git --locked devmap-cli")
    exit 1
}

$RepoRoot = Split-Path -Parent $PSScriptRoot

$argv = @(if ($Uninstall) { "uninstall" } else { "install" })
if ($Help) { $argv += "--help" }
if ($List) { $argv += "--list" }
if ($HostOnly) { $argv += "host" }
$argv += $Components
if ($All) { $argv += "--all" }
if ($Yes) { $argv += "--yes" }
if ($DryRun) { $argv += "--dry-run" }
if ($Prefix) { $argv += @("--prefix", $Prefix) }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("devcouncil-bootstrap-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
$code = 1
try {
    $hostExe = Join-Path $tmp "devcouncil.exe"
    go -C (Join-Path $RepoRoot "backend\go_orchestrator") build -o $hostExe ./cmd/devcouncil
    if ($LASTEXITCODE -ne 0) {
        $code = $LASTEXITCODE
    } else {
        $env:DEVCOUNCIL_ROOT = $RepoRoot
        & $hostExe @argv
        $code = $LASTEXITCODE
    }
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
exit $code
