#!/usr/bin/env bash
set -euo pipefail

# ─────────────────────────────────────────────────────────────────────
# llm-router  启动脚本
# 用法：bash deploy/deploy-ubuntu.sh
# 前提：从 GitHub clone 完整仓库（需包含 release/llm-router）
# ─────────────────────────────────────────────────────────────────────

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."

export ROUTER_BIND="${BIND_ADDR:-0.0.0.0:8080}"
export ROUTER_CONFIG="${ROUTER_CONFIG:-config/router.json}"
export RUST_LOG="${RUST_LOG:-info}"

echo "═══════════════════════════════════════════════"
echo "  llm-router"
echo "  BIND:   ${ROUTER_BIND}"
echo "  CONFIG: ${ROUTER_CONFIG}"
echo "═══════════════════════════════════════════════"
echo ""

exec ./release/llm-router
