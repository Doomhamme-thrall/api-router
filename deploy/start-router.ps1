param(
    [string]$Bind = "127.0.0.1:8080",
    [string]$ConfigPath = "config/router.json",
    [switch]$SkipBuild,
    [switch]$OpenUi
)

$ErrorActionPreference = "Stop"

function Resolve-CargoPath {
    $candidates = @(
        "$env:USERPROFILE\.cargo\bin\cargo.exe",
        "C:\Program Files\Rust\bin\cargo.exe",
        "C:\Rust\bin\cargo.exe"
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cmd) {
        return $cmd.Source
    }

    throw "Cargo not found. Install Rust toolchain first."
}

function Assert-Config([string]$Path) {
    if (-not (Test-Path $Path)) {
        throw "Config file not found: $Path"
    }

    $raw = Get-Content -Path $Path -Raw
    try {
        $cfg = $raw | ConvertFrom-Json
    }
    catch {
        throw "Config is not valid JSON: $Path"
    }

    if (-not $cfg.jwt_secret) {
        throw "Missing jwt_secret in config."
    }

    if (-not $cfg.targets -or $cfg.targets.Count -eq 0) {
        Write-Warning "No targets found in config. Router can start but cannot proxy requests."
    }
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")
Set-Location $repoRoot

$cargoPath = Resolve-CargoPath
Assert-Config -Path $ConfigPath

$env:ROUTER_BIND = $Bind
$env:ROUTER_CONFIG = (Resolve-Path $ConfigPath).Path
if (-not $env:RUST_LOG) {
    $env:RUST_LOG = "info"
}

Write-Host "==> Repo Root: $repoRoot"
Write-Host "==> Cargo: $cargoPath"
Write-Host "==> ROUTER_BIND: $env:ROUTER_BIND"
Write-Host "==> ROUTER_CONFIG: $env:ROUTER_CONFIG"

if (-not $SkipBuild) {
    Write-Host "==> Running cargo check..."
    & $cargoPath check
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

if ($OpenUi) {
    Start-Job -ScriptBlock {
        Start-Sleep -Seconds 2
        Start-Process "http://127.0.0.1:8080/ui"
    } | Out-Null
}

Write-Host "==> Starting router (Ctrl+C to stop)..."
& $cargoPath run
exit $LASTEXITCODE
