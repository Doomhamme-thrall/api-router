<#
.SYNOPSIS
  llm-router Windows 一键部署脚本
.DESCRIPTION
  自动安装依赖、构建前后端、注册 Windows 服务、启动服务。
  以管理员身份运行:  PowerShell -ExecutionPolicy Bypass .\deploy\deploy-windows.ps1
.PARAMETER InstallDir
  安装目录，默认 C:\llm-router
.PARAMETER BindAddr
  监听地址，默认 127.0.0.1:8080
.PARAMETER SkipBuild
  跳过构建步骤
.PARAMETER Uninstall
  卸载服务并删除安装目录
#>

param(
    [string]$InstallDir    = "C:\llm-router",
    [string]$BindAddr      = "127.0.0.1:8080",
    [switch]$SkipBuild,
    [switch]$Uninstall
)

#Requires -RunAsAdministrator

$ErrorActionPreference = "Stop"
$Host.UI.RawUI.WindowTitle = "llm-router Windows 一键部署"

function Write-Info  { Write-Host "[INFO] $args" -ForegroundColor Cyan }
function Write-Ok    { Write-Host "[OK]   $args" -ForegroundColor Green }
function Write-Warn  { Write-Host "[WARN] $args" -ForegroundColor Yellow }
function Write-Err   { Write-Host "[ERR]  $args" -ForegroundColor Red }

# ──────────────────────────────────────────────────────────────────
# 卸载模式
# ──────────────────────────────────────────────────────────────────
if ($Uninstall) {
    Write-Info "卸载 llm-router..."

    $service = Get-Service -Name "llm-router" -ErrorAction SilentlyContinue
    if ($service) {
        Stop-Service "llm-router" -Force -ErrorAction SilentlyContinue
        & nssm stop "llm-router" 2>$null
        & nssm remove "llm-router" confirm 2>$null
        sc.exe delete "llm-router" 2>$null
        Write-Ok "服务已卸载"
    }

    if (Test-Path $InstallDir) {
        Remove-Item -Recurse -Force $InstallDir
        Write-Ok "安装目录已删除: $InstallDir"
    }

    Write-Ok "卸载完成"
    return
}

# ──────────────────────────────────────────────────────────────────
# 0. 检查管理员权限
# ──────────────────────────────────────────────────────────────────
Write-Info "检查管理员权限..."
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Err "请以管理员身份运行 PowerShell！"
    Write-Err "右键 PowerShell -> 以管理员身份运行"
    exit 1
}
Write-Ok "管理员权限 ✓"

# ──────────────────────────────────────────────────────────────────
# 1. 检查系统依赖
# ──────────────────────────────────────────────────────────────────
Write-Info "检查系统依赖..."

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")
Set-Location $repoRoot

# Rust / Cargo
$cargoPath = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargoPath) {
    $cargoCandidates = @(
        "$env:USERPROFILE\.cargo\bin\cargo.exe",
        "C:\Program Files\Rust\bin\cargo.exe",
        "C:\Rust\bin\cargo.exe"
    )
    foreach ($c in $cargoCandidates) {
        if (Test-Path $c) { $cargoPath = Get-Command $c; break }
    }
}
if (-not $cargoPath) {
    Write-Warn "Rust/Cargo 未安装，是否自动安装？[Y/n]"
    $ans = Read-Host
    if ($ans -ne "" -and $ans -ne "Y" -and $ans -ne "y") {
        Write-Err "请手动安装 Rust: https://rustup.rs"
        exit 1
    }
    Write-Info "正在安装 Rust..."
    $rustup = "$env:TEMP\rustup-init.exe"
    Invoke-WebRequest -Uri "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe" -OutFile $rustup
    Start-Process -FilePath $rustup -ArgumentList "-y --default-host x86_64-pc-windows-msvc" -Wait
    Remove-Item $rustup -Force -ErrorAction SilentlyContinue
    # 刷新 PATH 环境变量
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "User") + ";$env:USERPROFILE\.cargo\bin"
    $cargoPath = Get-Command cargo -ErrorAction Stop
    Write-Ok "Rust 安装完成 ✓"
}
Write-Ok "Rust ✓  ($($cargoPath.Source))"

# Node.js
$nodePath = Get-Command node -ErrorAction SilentlyContinue
if (-not $nodePath) {
    Write-Warn "Node.js 未安装，是否自动安装？[Y/n]"
    $ans = Read-Host
    if ($ans -ne "" -and $ans -ne "Y" -and $ans -ne "y") {
        Write-Err "请手动安装 Node.js: https://nodejs.org"
        exit 1
    }
    Write-Info "正在安装 Node.js..."
    $nodeInstaller = "$env:TEMP\node-installer.msi"
    Invoke-WebRequest -Uri "https://nodejs.org/dist/v20.18.0/node-v20.18.0-x64.msi" -OutFile $nodeInstaller
    Start-Process -FilePath "msiexec.exe" -ArgumentList "/i `"$nodeInstaller`" /quiet /norestart" -Wait
    Remove-Item $nodeInstaller -Force -ErrorAction SilentlyContinue
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")
    $nodePath = Get-Command node -ErrorAction Stop
    Write-Ok "Node.js 安装完成 ✓"
}
Write-Ok "Node.js ✓  ($($nodePath.Source))"

# NSSM (Non-Sucking Service Manager) — 用于注册 Windows 服务
$nssmPath = Get-Command nssm -ErrorAction SilentlyContinue
if (-not $nssmPath) {
    # 从安装目录或当前目录查找
    $localNssm = Join-Path $scriptDir "nssm.exe"
    if (Test-Path $localNssm) { $nssmPath = $localNssm }
}
if (-not $nssmPath) {
    Write-Warn "NSSM 未安装（用于注册 Windows 服务）"
    Write-Warn "是否自动下载？[Y/n]"
    $ans = Read-Host
    if ($ans -eq "" -or $ans -eq "Y" -or $ans -eq "y") {
        Write-Info "正在下载 NSSM..."
        $nssmZip = "$env:TEMP\nssm.zip"
        $nssmExtract = "$env:TEMP\nssm"
        # NSSM 2.24 下载地址（latest stable）
        Invoke-WebRequest -Uri "https://nssm.cc/release/nssm-2.24.zip" -OutFile $nssmZip
        Expand-Archive -Path $nssmZip -DestinationPath $nssmExtract -Force
        $nssmExe = Get-ChildItem -Path $nssmExtract -Recurse -Filter "nssm.exe" | Where-Object { $_.DirectoryName -like "*win64*" } | Select-Object -First 1
        if ($nssmExe) {
            Copy-Item $nssmExe.FullName (Join-Path $scriptDir "nssm.exe") -Force
            $nssmPath = Join-Path $scriptDir "nssm.exe"
            Write-Ok "NSSM 下载完成 ✓"
        } else {
            Write-Warn "NSSM 下载失败，将跳过服务注册"
        }
        Remove-Item $nssmZip -Force -ErrorAction SilentlyContinue
        Remove-Item $nssmExtract -Recurse -Force -ErrorAction SilentlyContinue
    } else {
        Write-Warn "跳过 NSSM 安装，将不使用系统服务"
    }
}
if ($nssmPath) { Write-Ok "NSSM ✓" }

# ──────────────────────────────────────────────────────────────────
# 2. 构建前端
# ──────────────────────────────────────────────────────────────────
if (-not $SkipBuild) {
    Write-Info "构建前端 (ui/)..."
    Set-Location (Join-Path $repoRoot "ui")
    if (-not (Test-Path "node_modules")) {
        & npm install --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) { throw "npm install failed" }
    }
    & npm run build
    if ($LASTEXITCODE -ne 0) { throw "npm run build failed" }
    Write-Ok "前端构建完成 ✓"
    Set-Location $repoRoot
}

# ──────────────────────────────────────────────────────────────────
# 3. 构建后端
# ──────────────────────────────────────────────────────────────────
if (-not $SkipBuild) {
    Write-Info "构建后端 (cargo build --release)..."
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
    Write-Ok "后端构建完成 ✓"
}

# ──────────────────────────────────────────────────────────────────
# 4. 创建安装目录并拷贝文件
# ──────────────────────────────────────────────────────────────────
Write-Info "部署到 ${InstallDir}..."

# 停止已有服务
$existing = Get-Service -Name "llm-router" -ErrorAction SilentlyContinue
if ($existing) {
    Write-Warn "检测到已存在的 llm-router 服务，正在停止..."
    Stop-Service "llm-router" -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
}

# 创建目录
$dirs = @(
    $InstallDir,
    (Join-Path $InstallDir "config"),
    (Join-Path $InstallDir "data\usage"),
    (Join-Path $InstallDir "deploy")
)
foreach ($d in $dirs) {
    New-Item -ItemType Directory -Path $d -Force | Out-Null
}

# 拷贝 binary
Copy-Item (Join-Path $repoRoot "target\release\llm-router.exe") $InstallDir -Force
Write-Ok "binary -> $(Join-Path $InstallDir 'llm-router.exe')"

# 配置：保留已有，否则从项目根目录拷贝
$configDest = Join-Path $InstallDir "config\router.json"
$configSrc  = Join-Path $repoRoot "config\router.json"
if (Test-Path $configDest) {
    Write-Warn "配置文件已存在，跳过覆盖: $configDest"
} elseif (Test-Path $configSrc) {
    Copy-Item $configSrc $configDest -Force
    Write-Ok "配置 -> $configDest"
} else {
    # 创建默认配置
    $jwtSecret = -join ((48..57) + (97..102) | Get-Random -Count 32 | ForEach-Object { [char]$_ })
    @"
{
  "admin": {
    "username": "admin",
    "password_sha256": "240be518fabd2724ddb6f04eeb1da5967448d7e831c08c8fa822809f74c720a9"
  },
  "jwt_secret": "$jwtSecret",
  "client_api_keys": ["client-demo-key"],
  "targets": [],
  "model_groups": []
}
"@ | Set-Content $configDest -Encoding UTF8
    Write-Ok "已创建默认配置: $configDest"
}

# UI 静态文件
$uiDest = Join-Path $InstallDir "ui"
if (Test-Path $uiDest) { Remove-Item -Recurse -Force $uiDest }
Copy-Item -Recurse (Join-Path $repoRoot "ui\dist") $uiDest
Write-Ok "前端静态文件 -> $uiDest"

# 辅助脚本
Copy-Item (Join-Path $scriptDir "start-router.ps1") (Join-Path $InstallDir "deploy\") -Force
Write-Ok "辅助脚本 -> $(Join-Path $InstallDir 'deploy\')"

# ──────────────────────────────────────────────────────────────────
# 5. 注册 Windows 服务
# ──────────────────────────────────────────────────────────────────
if ($nssmPath) {
    Write-Info "注册 Windows 服务..."

    $exePath  = Join-Path $InstallDir "llm-router.exe"
    $workDir  = $InstallDir
    $logDir   = Join-Path $InstallDir "logs"
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null

    # 如果服务已存在，先删除
    & $nssmPath stop "llm-router" confirm 2>$null
    & $nssmPath remove "llm-router" confirm 2>$null

    # 安装服务
    & $nssmPath install "llm-router" $exePath
    & $nssmPath set "llm-router" AppDirectory $workDir
    & $nssmPath set "llm-router" AppStdout (Join-Path $logDir "stdout.log")
    & $nssmPath set "llm-router" AppStderr (Join-Path $logDir "stderr.log")
    & $nssmPath set "llm-router" AppEnvironmentExtra "ROUTER_BIND=$BindAddr" "ROUTER_CONFIG=$(Join-Path $InstallDir 'config\router.json')" "RUST_LOG=info"
    & $nssm_path set "llm-router" AppRotateFiles 1
    & $nssm_path set "llm-router" AppRotateSeconds 86400
    & $nssm_path set "llm-router" Start SERVICE_AUTO_START
    & $nssm_path set "llm-router" ObjectName "LocalSystem"

    # 启动服务
    & $nssmPath start "llm-router"

    Write-Ok "Windows 服务已注册并启动 ✓ (llm-router)"
} else {
    Write-Warn "NSSM 不可用，跳过服务注册。"
    Write-Warn "你可以手动运行:  start `"$exePath`""
}

# ──────────────────────────────────────────────────────────────────
# 6. 完成
# ──────────────────────────────────────────────────────────────────
Write-Info ""
Write-Info "═══════════════════════════════════════════════════════════"
Write-Ok  "部署完成！"
Write-Info "═══════════════════════════════════════════════════════════"
Write-Info ""
Write-Info "  安装目录:   $InstallDir"
Write-Info "  监听地址:   http://$BindAddr/"
Write-Info "  管理界面:   http://$BindAddr/ui/"
Write-Info "  健康检查:   http://$BindAddr/healthz"
Write-Info ""
if ($nssmPath) {
    Write-Info "  管理命令:"
    Write-Info "    nssm status llm-router     # 查看状态"
    Write-Info "    nssm restart llm-router    # 重启服务"
    Write-Info "    nssm stop llm-router       # 停止服务"
    Write-Info "    Get-Content $((Join-Path $InstallDir 'logs\stdout.log')) -Tail 50  # 查看日志"
}
Write-Info ""
Write-Info "  配置文件: $(Join-Path $InstallDir 'config\router.json')"
Write-Info "  日志目录: $(Join-Path $InstallDir 'data\usage\')"
Write-Info ""
Write-Info "  首次使用请编辑配置中的 API keys，然后重启服务。"
Write-Info ""
Write-Info "  卸载:"
Write-Info "    .\deploy\deploy-windows.ps1 -Uninstall"
Write-Info "═══════════════════════════════════════════════════════════"
