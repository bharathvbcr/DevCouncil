# First-time bootstrap for DevCouncil on Windows.
#
# `devcouncil install` is the installer. This script only gets it running on
# a machine with no host yet: it builds cmd/dcinstall — the same install code
# the host runs — into a temporary directory and runs it with install (or
# uninstall with -Uninstall), translating these parameters to its flags. Every preset and
# health check belongs to that command; -Help prints them.
#
#   .\scripts\install.ps1                          # host + every analysis binary
#   .\scripts\install.ps1 -Components devmap       # one component, no host
#   .\scripts\install.ps1 -Uninstall -Components dcgrep -Yes
#
# Without Go, devmap alone installs with (pinned to a release tag; the
# default branch is whatever was merged last)
#   cargo install --git https://github.com/bharathvbcr/DevCouncil.git --tag vX.Y.Z --locked devmap-cli
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

$RepoRoot = Split-Path -Parent $PSScriptRoot

if (-not (Get-Command go -ErrorAction SilentlyContinue)) {
    # The tag is this checkout's release: the constant the release gate holds
    # package.json, Cargo.toml and the vX.Y.Z tag to.
    $Tag = "vX.Y.Z"
    $VersionFile = Join-Path $RepoRoot "backend\go_orchestrator\devcouncil\version\version.go"
    $Match = Select-String -LiteralPath $VersionFile -Pattern '^const Version = "([^"]+)"$' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($Match) { $Tag = "v" + $Match.Matches[0].Groups[1].Value }
    [Console]::Error.WriteLine("go is required: this script builds the DevCouncil host, and the host is the installer.")
    [Console]::Error.WriteLine("Without Go, install devmap alone: cargo install --git https://github.com/bharathvbcr/DevCouncil.git --tag $Tag --locked devmap-cli")
    exit 1
}

$argv = @(if ($Uninstall) { "uninstall" } else { "install" })
if ($Help) { $argv += "--help" }
if ($List) { $argv += "--list" }
if ($HostOnly) { $argv += "host" }
$argv += $Components
# --all and --yes are uninstall's; install refuses flags it does not know.
if ($Uninstall -and $All) { $argv += "--all" }
if ($Uninstall -and $Yes) { $argv += "--yes" }
# DRY_RUN=1 is how the scripts this replaced were asked to touch nothing.
if ($DryRun -or ($env:DRY_RUN -and $env:DRY_RUN -ne "0")) { $argv += "--dry-run" }
if ($Prefix) { $argv += @("--prefix", $Prefix) }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("devcouncil-bootstrap-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
$code = 1
try {
    $hostExe = Join-Path $tmp "dcinstall.exe"
    go -C (Join-Path $RepoRoot "backend\go_orchestrator") build -o $hostExe ./cmd/dcinstall
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
