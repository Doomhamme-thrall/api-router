<#
.SYNOPSIS
  llm-router 启动脚本
.DESCRIPTION
  从 GitHub clone 的完整仓库中直接启动预编译二进制。
  用法: PowerShell .\deploy\deploy-windows.ps1
#>

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")
Set-Location $repoRoot

if (-not $env:ROUTER_BIND) { $env:ROUTER_BIND = "127.0.0.1:8080" }
if (-not $env:ROUTER_CONFIG) { $env:ROUTER_CONFIG = "config/router.json" }
if (-not $env:RUST_LOG) { $env:RUST_LOG = "info" }

Write-Host "═══════════════════════════════════════════════"
Write-Host "  llm-router"
Write-Host "  BIND:   $env:ROUTER_BIND"
Write-Host "  CONFIG: $env:ROUTER_CONFIG"
Write-Host "═══════════════════════════════════════════════"
Write-Host ""

& ".\release\llm-router.exe"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
