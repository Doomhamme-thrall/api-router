#!/usr/bin/env bash
set -euo pipefail

# ─────────────────────────────────────────────────────────────────────
# llm-router  Ubuntu/Debian 一键部署脚本
# 用法：sudo bash deploy/deploy-ubuntu.sh
#
# 默认使用仓库中预编译的二进制 (release/llm-router + release/ui)，
# 无需安装 Rust 或 Node.js 即可部署。
# 如需从源码编译，设置 SKIP_BUILD=0 即可。
# ─────────────────────────────────────────────────────────────────────

# ---------- 颜色 ----------
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; NC='\033[0m'
info()  { echo -e "${CYAN}[INFO]${NC} $*"; }
ok()    { echo -e "${GREEN}[OK]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
err()   { echo -e "${RED}[ERR]${NC} $*" >&2; }

# ---------- 配置 ----------
INSTALL_DIR="${INSTALL_DIR:-/opt/llm-router}"
BIND_ADDR="${BIND_ADDR:-127.0.0.1:8080}"
NGINX_SITE="${NGINX_SITE:-llm-router}"
# 默认跳过编译（使用 release/ 预编译二进制）
SKIP_BUILD="${SKIP_BUILD:-1}"
SKIP_NGINX="${SKIP_NGINX:-0}"

# ---------- 检查 root ----------
if [[ $EUID -ne 0 ]]; then
  err "请以 root 身份运行:  sudo bash deploy/deploy-ubuntu.sh"
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

echo "═══════════════════════════════════════════════════════════"
echo "   llm-router Ubuntu/Debian 一键部署"
echo "═══════════════════════════════════════════════════════════"
echo "  源目录:     ${REPO_ROOT}"
echo "  安装目录:   ${INSTALL_DIR}"
echo "  监听地址:   ${BIND_ADDR}"
echo "  编译模式:   $([[ "${SKIP_BUILD}" == "1" ]] && echo '使用预编译二进制' || echo '从源码编译')"
echo ""

# ================================================================
# 0. 检查系统依赖（仅基础工具，不需要 Rust/Node.js）
# ================================================================
info "检查系统依赖..."
DEPS_MISSING=()
for cmd in curl git jq; do
  command -v "$cmd" &>/dev/null || DEPS_MISSING+=("$cmd")
done

if [[ ${#DEPS_MISSING[@]} -gt 0 ]]; then
  warn "缺少: ${DEPS_MISSING[*]}"
  apt-get update -qq && apt-get install -y -qq "${DEPS_MISSING[@]}"
  ok "系统依赖安装完成 ✓"
fi

# 开发者模式：检查 Rust/Node.js
if [[ "${SKIP_BUILD}" == "0" ]]; then
  if ! command -v cargo &>/dev/null; then
    err "SKIP_BUILD=0 但未安装 cargo。请安装 Rust 或设置 SKIP_BUILD=1 使用预编译二进制。"
    exit 1
  fi
  if ! command -v node &>/dev/null || ! command -v npm &>/dev/null; then
    err "SKIP_BUILD=0 但未安装 Node.js/npm。请安装或设置 SKIP_BUILD=1。"
    exit 1
  fi
fi

ok "系统依赖检查通过"

# ================================================================
# 1. （可选）从源码编译
# ================================================================
if [[ "${SKIP_BUILD}" == "0" ]]; then
  info "编译前端 (ui/)..."
  (cd ui && npm ci && npm run build)
  ok "前端编译完成 ✓"

  info "编译后端 (cargo build --release)..."
  cargo build --release
  ok "后端编译完成 ✓"
fi

# ================================================================
# 2. 验证预编译文件
# ================================================================
if [[ ! -f "release/llm-router" ]]; then
  err "未找到预编译二进制: release/llm-router"
  echo "  - 首次使用请先推送代码触发 CI，CI 会自动编译并提交二进制到 release/"
  echo "  - 或设置 SKIP_BUILD=0 从源码编译"
  exit 1
fi
if [[ ! -d "release/ui" ]]; then
  err "未找到前端静态文件: release/ui/"
  echo "  - 请先推送代码触发 CI 构建，或设置 SKIP_BUILD=0 从源码编译"
  exit 1
fi

# ================================================================
# 3. 创建安装目录并拷贝文件
# ================================================================
info "部署到 ${INSTALL_DIR}..."

mkdir -p "${INSTALL_DIR}/config"
mkdir -p "${INSTALL_DIR}/data/usage"
mkdir -p "${INSTALL_DIR}/deploy"

# binary（来自 release/ 预编译二进制）
cp "release/llm-router" "${INSTALL_DIR}/llm-router"
chmod 755 "${INSTALL_DIR}/llm-router"
ok "binary -> ${INSTALL_DIR}/llm-router"

# 配置：保留现有配置，否则从仓库拷贝或创建默认
if [[ -f "${INSTALL_DIR}/config/router.json" ]]; then
  warn "配置文件已存在，跳过覆盖: ${INSTALL_DIR}/config/router.json"
else
  if [[ -f "${REPO_ROOT}/config/router.json" ]]; then
    cp "${REPO_ROOT}/config/router.json" "${INSTALL_DIR}/config/router.json"
  else
    JWT_SECRET="${JWT_SECRET:-$(openssl rand -hex 32)}"
    cat > "${INSTALL_DIR}/config/router.json" <<JSONCFG
{
  "admin": {
    "username": "admin",
    "password_sha256": "240be518fabd2724ddb6f04eeb1da5967448d7e831c08c8fa822809f74c720a9"
  },
  "jwt_secret": "${JWT_SECRET}",
  "client_api_keys": ["client-demo-key"],
  "targets": [],
  "model_groups": []
}
JSONCFG
    ok "已创建默认配置（请修改 jwt_secret 和 API keys）"
  fi
fi
# 补全 jwt_secret 如果为空
if [[ "$(jq -r '.jwt_secret // empty' "${INSTALL_DIR}/config/router.json")" == "" ]]; then
  NEW_SECRET="$(openssl rand -hex 32)"
  jq ".jwt_secret = \"${NEW_SECRET}\"" "${INSTALL_DIR}/config/router.json" > /tmp/router.json.tmp
  mv /tmp/router.json.tmp "${INSTALL_DIR}/config/router.json"
  ok "已生成 jwt_secret"
fi

# UI（来自 release/ 预编译前端）
rm -rf "${INSTALL_DIR}/ui"
cp -r "release/ui" "${INSTALL_DIR}/ui"
chmod -R 755 "${INSTALL_DIR}/ui"
ok "前端静态文件 -> ${INSTALL_DIR}/ui/"

# 辅助脚本
cp "${SCRIPT_DIR}/run.sh" "${INSTALL_DIR}/deploy/"
chmod 755 "${INSTALL_DIR}/deploy/run.sh"
ok "辅助脚本 -> ${INSTALL_DIR}/deploy/"

# 权限
chown -R www-data:www-data "${INSTALL_DIR}" 2>/dev/null || true

# ================================================================
# 4. 安装 systemd 服务
# ================================================================
info "配置 systemd 服务..."

cat > /etc/systemd/system/llm-router.service <<UNIT
[Unit]
Description=LLM Router Service
After=network.target

[Service]
Type=simple
User=www-data
WorkingDirectory=${INSTALL_DIR}
ExecStart=${INSTALL_DIR}/llm-router
Restart=always
RestartSec=3
Environment=ROUTER_BIND=${BIND_ADDR}
Environment=ROUTER_CONFIG=${INSTALL_DIR}/config/router.json
Environment=RUST_LOG=info
NoNewPrivileges=true
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable llm-router
systemctl restart llm-router
ok "systemd 服务安装并启动 ✓"

# ================================================================
# 5. 配置 Nginx（可选）
# ================================================================
if [[ "${SKIP_NGINX}" != "1" ]] && command -v nginx &>/dev/null; then
  info "配置 Nginx..."

  cat > /etc/nginx/sites-available/${NGINX_SITE} <<NGINX
server {
    listen 80;
    server_name _;

    location / {
        proxy_pass http://${BIND_ADDR};
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_read_timeout 3600;
        proxy_send_timeout 3600;
    }
}
NGINX

  if [[ -f "/etc/nginx/sites-enabled/default" ]]; then
    rm -f /etc/nginx/sites-enabled/default
  fi
  ln -sf "/etc/nginx/sites-available/${NGINX_SITE}" "/etc/nginx/sites-enabled/${NGINX_SITE}"

  if nginx -t 2>/dev/null; then
    systemctl reload nginx || systemctl restart nginx
    ok "Nginx 配置完成 ✓"
  else
    warn "Nginx 配置测试失败，请手动检查: nginx -t"
  fi
else
  info "跳过 Nginx 配置 (SKIP_NGINX=1 或 Nginx 未安装)"
fi

# ================================================================
# 6. 完成
# ================================================================
echo ""
echo "═══════════════════════════════════════════════════════════"
echo -e "  ${GREEN}部署完成！${NC}"
echo "═══════════════════════════════════════════════════════════"
echo ""
echo "  服务状态:"
systemctl --no-pager status llm-router 2>&1 | head -5
echo ""
echo "  访问地址:"
echo "    HTTP:      http://$(curl -s ifconfig.me 2>/dev/null || echo 'your-server-ip')/"
echo "    管理界面:  http://$(curl -s ifconfig.me 2>/dev/null || echo 'your-server-ip')/ui/"
echo "    健康检查:  http://$(curl -s ifconfig.me 2>/dev/null || echo 'your-server-ip')/healthz"
echo ""
echo "  管理命令:"
echo "    sudo systemctl status llm-router      # 查看状态"
echo "    sudo systemctl restart llm-router     # 重启"
echo "    sudo journalctl -u llm-router -f      # 查看实时日志"
echo ""
echo "  配置文件: ${INSTALL_DIR}/config/router.json"
echo "  日志目录: ${INSTALL_DIR}/data/usage/"
echo ""
echo "  首次使用请编辑配置文件并重启服务。"
echo "═══════════════════════════════════════════════════════════"
