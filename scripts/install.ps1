param(
    [string]$Prefix = "$env:USERPROFILE\.local",
    [string[]]$Components = @(),
    [switch]$Uninstall,
    [switch]$All,
    [switch]$Help,
    [switch]$HostOnly,
    [switch]$Yes,
    [switch]$List
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Write-Host @"
Usage: .\scripts\install.ps1 [options]

Install DevCouncil components into `$Prefix\bin (default: ~\.local\bin).

  -Components NAME,...   subset: host, devmap, dcstore, dcverify, dcgrep, analysis, all
  -HostOnly              Go host only
  -Uninstall             remove named binaries
  -All                   with -Uninstall, remove every catalog binary
  -Yes                   skip uninstall confirmation
  -List                  print the catalog
  -Prefix DIR            install root
  -Help                  this help

Standalone code intelligence:
  .\scripts\install.ps1 -Components devmap

There is no uv / Python install path. These are native Go and Rust binaries.
"@
    exit 0
}

if ($List) {
    Write-Host "presets: all analysis codeintel devmap host"
    Write-Host "components: host devmap dcstore dcverify dcgrep"
    exit 0
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$bindir = Join-Path $Prefix "bin"

$goOut = Join-Path $bindir "devcouncil.exe"
$devExe = Join-Path $bindir "dev.exe"

$WantHost = $true
$RustNames = @("dcstore", "dcverify", "dcgrep", "devmap")
if ($HostOnly) {
    $WantHost = $true
    $RustNames = @()
} elseif ($Components.Count -gt 0) {
    $WantHost = $false
    $RustNames = @()
    foreach ($c in $Components) {
        switch -Regex ($c.ToLowerInvariant()) {
            '^(all)$' { $WantHost = $true; $RustNames = @("dcstore", "dcverify", "dcgrep", "devmap") }
            '^(host)$' { $WantHost = $true }
            '^(analysis)$' { $RustNames = @("dcstore", "dcverify", "dcgrep", "devmap") }
            '^(codeintel|devmap)$' { $RustNames += "devmap" }
            '^(dcstore|dcverify|dcgrep)$' { $RustNames += $c.ToLowerInvariant() }
            default { throw "unknown component '$c' (see -Help)" }
        }
    }
    $RustNames = @($RustNames | Select-Object -Unique)
}

if ($Uninstall) {
    if ($All -and $Components.Count -gt 0) {
        Write-Error "do not mix -All with -Components (see -Help)"
        exit 2
    }
    if (-not $All -and $Components.Count -eq 0 -and -not $HostOnly) {
        Write-Error "uninstall requires -Components NAME or -All (see -Help)"
        exit 2
    }
    if ($All) {
        $WantHost = $true
        $RustNames = @("dcstore", "dcverify", "dcgrep", "devmap")
    }
    $targets = @()
    if ($WantHost) { $targets += @("devcouncil.exe", "dev.exe") }
    foreach ($n in $RustNames) { $targets += "$n.exe" }
    if (-not $Yes) {
        Write-Error "This will remove from $bindir : $($targets -join ', '). Re-run with -Yes to confirm."
        exit 2
    }
    foreach ($t in $targets) {
        $p = Join-Path $bindir $t
        $item = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
        if (-not $item) { continue }
        if ($item.PSIsContainer -and $item.LinkType -notin @('Junction', 'SymbolicLink')) {
            Write-Host "note: left directory $p in place"
            continue
        }
        Remove-Item -LiteralPath $p -Force
        Write-Host "removed $p"
    }
    exit 0
}

New-Item -ItemType Directory -Force -Path $bindir | Out-Null

if ($WantHost -and -not (Get-Command go -ErrorAction SilentlyContinue)) {
    Write-Error "go is required to install the host. Install a Go toolchain, or pass -Components devmap to skip it."
    exit 1
}

# `dev.exe` is our host under PATHEXT. Refuse a foreign `dev.exe` — Shopify's
# CLI, a personal script, or a directory of that name. A previous Copy-Item of
# our `devcouncil.exe` is ours; a symlink/junction is ours only when its target
# basename is devcouncil or devcouncil.exe. Probe before `go build` overwrites
# devcouncil.exe so a stale copy can still match.
function Get-DevReplaceBlocker {
    param(
        [Parameter(Mandatory = $true)][string]$Dest,
        [Parameter(Mandatory = $true)][string]$HostExe
    )
    $item = Get-Item -LiteralPath $Dest -Force -ErrorAction SilentlyContinue
    if (-not $item) {
        return $null
    }
    if ($item.PSIsContainer -and $item.LinkType -notin @('Junction', 'SymbolicLink')) {
        return "refusing to replace $Dest (it is a directory). The Go host is $HostExe."
    }
    $isLink = ($item.LinkType -in @('SymbolicLink', 'Junction')) -or
        (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
    if ($isLink) {
        $targets = @($item.Target)
        foreach ($t in $targets) {
            if ([string]::IsNullOrWhiteSpace($t)) { continue }
            $base = [System.IO.Path]::GetFileName($t.TrimEnd('\', '/'))
            if ($base -eq 'devcouncil' -or $base -eq 'devcouncil.exe') {
                return $null
            }
        }
        $shown = if ($targets.Count -gt 0) { $targets -join ', ' } else { 'unknown' }
        return "refusing to replace $Dest (symlink to $shown, not devcouncil). The Go host is $HostExe."
    }
    if (Test-Path -LiteralPath $HostExe -PathType Leaf) {
        $destHash = (Get-FileHash -LiteralPath $Dest).Hash
        $hostHash = (Get-FileHash -LiteralPath $HostExe).Hash
        if ($destHash -eq $hostHash) {
            return $null
        }
    }
    return "refusing to replace $Dest (not a symlink/junction to our binary). The Go host is $HostExe."
}

if ($WantHost) {
    $devBlocker = Get-DevReplaceBlocker -Dest $devExe -HostExe $goOut

    Write-Host "building Go host binary (devcouncil)"
    Push-Location (Join-Path $RepoRoot "backend\go_orchestrator")
    try {
        go build -o $goOut ./cmd/devcouncil
        if ($LASTEXITCODE -ne 0) { throw "go build failed with exit $LASTEXITCODE" }
    } finally {
        Pop-Location
    }

    if ($devBlocker) {
        Write-Error $devBlocker
        exit 1
    }

    $existingDev = Get-Item -LiteralPath $devExe -Force -ErrorAction SilentlyContinue
    if ($existingDev -and (($existingDev.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)) {
        Remove-Item -LiteralPath $devExe -Force
    }
    Copy-Item -LiteralPath $goOut -Destination $devExe -Force
    Write-Host "installed $goOut and $devExe"
}

function Install-RustComponents {
    param([string[]]$Names)
    if ($Names.Count -eq 0) { return }
    $installer = Join-Path $RepoRoot "scripts\install-components.sh"
    $bash = Get-Command bash -ErrorAction SilentlyContinue
    if ($bash) {
        $env:PREFIX = $Prefix
        & bash $installer @Names
        if ($LASTEXITCODE -ne 0) { throw "install-components.sh failed with exit $LASTEXITCODE" }
        return
    }

    Write-Host "bash not found; installing Rust components with cargo (health checks skipped)"
    $profileName = if ($env:PROFILE -eq "debug") { "debug" } else { "release" }
    $all = @(
        @{ Name = "dcstore"; Workspace = "rust"; Package = "dc-store" },
        @{ Name = "dcverify"; Workspace = "rust"; Package = "dc-verify" },
        @{ Name = "dcgrep"; Workspace = "rust"; Package = "dc-grep" },
        @{ Name = "devmap"; Workspace = "rust"; Package = "devmap-cli" }
    )
    $components = $all | Where-Object { $Names -contains $_.Name }
    foreach ($c in $components) {
        $ws = Join-Path $RepoRoot $c.Workspace
        Push-Location $ws
        try {
            $cargoArgs = if ($profileName -eq "release") {
                @("build", "--release", "-p", $c.Package, "--bin", $c.Name)
            } else {
                @("build", "-p", $c.Package, "--bin", $c.Name)
            }
            Write-Host "building $($c.Name)"
            & cargo @cargoArgs
            if ($LASTEXITCODE -ne 0) { throw "cargo build $($c.Name) failed with exit $LASTEXITCODE" }
        } finally {
            Pop-Location
        }
        $builtExe = Join-Path $ws "target\$profileName\$($c.Name).exe"
        $builtBare = Join-Path $ws "target\$profileName\$($c.Name)"
        $built = if (Test-Path -LiteralPath $builtExe) { $builtExe } else { $builtBare }
        if (-not (Test-Path -LiteralPath $built)) {
            throw "expected $builtExe after cargo build"
        }
        Copy-Item -LiteralPath $built -Destination (Join-Path $bindir "$($c.Name).exe") -Force
        Write-Host "installed $($c.Name) -> $(Join-Path $bindir "$($c.Name).exe")"
    }
}

if ($RustNames.Count -gt 0) {
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-Host "installing Rust analysis components"
        Install-RustComponents -Names $RustNames
    } else {
        [Console]::Error.WriteLine("note: cargo is not on PATH; skipped analysis components. Install a Rust toolchain and rerun, or run scripts/install-components.sh from Git Bash.")
    }
}

$bindirNorm = $bindir.TrimEnd('\', '/')
$bindirOnPath = $false
foreach ($part in ($env:PATH -split ';')) {
    if ([string]::IsNullOrWhiteSpace($part)) { continue }
    $norm = $part.Trim().Trim('"').TrimEnd('\', '/')
    if ([string]::Equals($norm, $bindirNorm, [System.StringComparison]::OrdinalIgnoreCase)) {
        $bindirOnPath = $true
        break
    }
}
if (-not $bindirOnPath) {
    [Console]::Error.WriteLine("note: $bindir is not on PATH. Add it with: `$env:PATH = `"$bindir;`$env:PATH`"")
}

Write-Host "Try: dev --help"
Write-Host "     devmap --version"
