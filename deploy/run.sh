#!/usr/bin/env bash
set -euo pipefail

BIND_ADDR="${BIND_ADDR:-0.0.0.0:8080}"
CONFIG_PATH="${CONFIG_PATH:-config/router.json}"
RUST_LOG="${RUST_LOG:-info}"
SKIP_BUILD="${SKIP_BUILD:-0}"
MODE="${MODE:-auto}"  # auto | cargo | binary
BINARY_PATH="${BINARY_PATH:-./llm-router}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

if [[ ! -f "${CONFIG_PATH}" ]]; then
  echo "[ERROR] Config file not found: ${CONFIG_PATH}" >&2
  echo "[HINT] Copy config/router.example.json to config/router.json and fill values." >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "[WARN] jq not found. Skipping JSON field validation."
else
  if [[ -z "$(jq -r '.jwt_secret // empty' "${CONFIG_PATH}")" ]]; then
    echo "[ERROR] jwt_secret is missing in ${CONFIG_PATH}" >&2
    exit 1
  fi
fi

export ROUTER_BIND="${BIND_ADDR}"
export ROUTER_CONFIG="${REPO_ROOT}/${CONFIG_PATH}"
export RUST_LOG

echo "==> REPO_ROOT=${REPO_ROOT}"
echo "==> ROUTER_BIND=${ROUTER_BIND}"
echo "==> ROUTER_CONFIG=${ROUTER_CONFIG}"
echo "==> MODE=${MODE}"

run_cargo_with_lockfile_compat() {
  local cargo_args=("$@")
  local err_log
  err_log="$(mktemp)"

  # First attempt with existing lockfile.
  if cargo "${cargo_args[@]}" 2> >(tee "${err_log}" >&2); then
    rm -f "${err_log}"
    return 0
  fi

  # Compatibility fallback for older cargo that cannot parse lockfile v4.
  if grep -q "lock file version 4 requires" "${err_log}"; then
    echo "[WARN] Detected Cargo.lock v4 incompatibility with current cargo." >&2
    echo "[WARN] Re-generating lockfile using local cargo version..." >&2
    rm -f Cargo.lock
    cargo generate-lockfile
    cargo "${cargo_args[@]}"
    rm -f "${err_log}"
    return 0
  fi

  rm -f "${err_log}"
  return 1
}

run_with_cargo() {
  if ! command -v cargo >/dev/null 2>&1; then
    echo "[ERROR] cargo not found. Install Rust toolchain or use MODE=binary." >&2
    exit 1
  fi

  if [[ "${SKIP_BUILD}" != "1" ]]; then
    echo "==> Running cargo check..."
    run_cargo_with_lockfile_compat check
  fi

  echo "==> Starting router with cargo run"
  run_cargo_with_lockfile_compat run
}

run_with_binary() {
  if [[ ! -x "${BINARY_PATH}" ]]; then
    echo "[ERROR] Binary not found or not executable: ${BINARY_PATH}" >&2
    echo "[HINT] Build with: cargo build --release && cp target/release/llm-router ./llm-router" >&2
    exit 1
  fi

  echo "==> Starting router binary: ${BINARY_PATH}"
  exec "${BINARY_PATH}"
}

case "${MODE}" in
  cargo)
    run_with_cargo
    ;;
  binary)
    run_with_binary
    ;;
  auto)
    if command -v cargo >/dev/null 2>&1; then
      run_with_cargo
    else
      run_with_binary
    fi
    ;;
  *)
    echo "[ERROR] Invalid MODE=${MODE}. Use auto | cargo | binary" >&2
    exit 1
    ;;
esac
