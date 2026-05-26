#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "run as root"
  exit 1
fi

export PATH="/root/.cargo/bin:${PATH}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found, run scripts/setup.sh first"
  exit 1
fi

BIN_PATH="${BIN_PATH:-/usr/local/bin/hpx}"
CONF_PATH="${CONF_PATH:-/etc/hpx/hpx.conf}"
SERVICE_NAME="${SERVICE_NAME:-hpx}"

if [[ ! -f "${CONF_PATH}" ]]; then
  echo "config not found at ${CONF_PATH}, run scripts/setup.sh first"
  exit 1
fi

if [[ ! -f "/etc/systemd/system/${SERVICE_NAME}.service" ]]; then
  echo "systemd service not found, run scripts/setup.sh first"
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${REPO_ROOT}"

cargo build --release
install -m 0755 "${REPO_ROOT}/target/release/hpx" "${BIN_PATH}"
systemctl restart "${SERVICE_NAME}"
systemctl status "${SERVICE_NAME}" --no-pager -l
