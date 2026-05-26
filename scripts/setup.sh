#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "run as root"
  exit 1
fi

dnf -y install git ca-certificates curl gcc gcc-c++ make cmake perl pkgconf-pkg-config

if ! command -v rustup >/dev/null 2>&1; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y
fi

export PATH="/root/.cargo/bin:${PATH}"

RUST_TOOLCHAIN="${RUST_TOOLCHAIN:-1.83.0}"
rustup toolchain install "${RUST_TOOLCHAIN}"
rustup default "${RUST_TOOLCHAIN}"

BIN_PATH="${BIN_PATH:-/usr/local/bin/hpx}"
CONF_PATH="${CONF_PATH:-/etc/hpx/hpx.conf}"
SERVICE_PATH="${SERVICE_PATH:-/etc/systemd/system/hpx.service}"
DOMAIN="${DOMAIN:-}"

if [[ -z "${DOMAIN}" && -f "${CONF_PATH}" ]]; then
  DOMAIN="$(sed -n 's/^host=//p' "${CONF_PATH}" | head -n 1 | tr -d '\r\n' || true)"
fi

if [[ -z "${DOMAIN}" ]]; then
  echo "DOMAIN is required on first install (example: DOMAIN=zyko2.online)"
  exit 1
fi

install -d -m 0755 /etc/hpx

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${REPO_ROOT}"

cargo build --release
install -m 0755 "${REPO_ROOT}/target/release/hpx" "${BIN_PATH}"

if [[ ! -f "${CONF_PATH}" ]]; then
  cat > "${CONF_PATH}" <<EOF
listen=0.0.0.0:443
cert=/root/.acme.sh/${DOMAIN}_ecc/fullchain.cer
key=/root/.acme.sh/${DOMAIN}_ecc/${DOMAIN}.key
host=${DOMAIN}
path=/path
uuid=00000000-0000-0000-0000-000000000000
connect_timeout_ms=5000
idle_timeout_s=1800
sub_path=/sub
sub_token=change_me
public_host=${DOMAIN}
public_port=443
EOF
  chmod 0644 "${CONF_PATH}"
fi

grep -q '^sub_path=' "${CONF_PATH}" || echo 'sub_path=/sub' >> "${CONF_PATH}"
grep -q '^sub_token=' "${CONF_PATH}" || echo 'sub_token=change_me' >> "${CONF_PATH}"
grep -q '^public_host=' "${CONF_PATH}" || echo "public_host=${DOMAIN}" >> "${CONF_PATH}"
grep -q '^public_port=' "${CONF_PATH}" || echo 'public_port=443' >> "${CONF_PATH}"

cat > "${SERVICE_PATH}" <<'EOF'
[Unit]
Description=hpx (VLESS over H2+TLS)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/hpx --config /etc/hpx/hpx.conf
Restart=always
RestartSec=1
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable hpx
systemctl restart hpx
systemctl status hpx --no-pager -l

echo
echo "config: ${CONF_PATH}"
echo "service: ${SERVICE_PATH}"
echo "acme path assumed:"
echo "  /root/.acme.sh/${DOMAIN}_ecc/${DOMAIN}.key"
echo "  /root/.acme.sh/${DOMAIN}_ecc/fullchain.cer"

SUB_PATH="$(sed -n 's/^sub_path=//p' "${CONF_PATH}" | head -n 1 | tr -d '\r\n' || true)"
SUB_TOKEN="$(sed -n 's/^sub_token=//p' "${CONF_PATH}" | head -n 1 | tr -d '\r\n' || true)"
PUBLIC_HOST="$(sed -n 's/^public_host=//p' "${CONF_PATH}" | head -n 1 | tr -d '\r\n' || true)"
PUBLIC_PORT="$(sed -n 's/^public_port=//p' "${CONF_PATH}" | head -n 1 | tr -d '\r\n' || true)"

if [[ -z "${PUBLIC_HOST}" ]]; then
  PUBLIC_HOST="${DOMAIN}"
fi
if [[ -z "${PUBLIC_PORT}" ]]; then
  PUBLIC_PORT="443"
fi

if [[ -n "${SUB_PATH}" ]]; then
  echo "subscription enabled:"
  echo "  clash: https://${PUBLIC_HOST}:${PUBLIC_PORT}${SUB_PATH}?fmt=clash"
  echo "  vless: https://${PUBLIC_HOST}:${PUBLIC_PORT}${SUB_PATH}?fmt=vless"
  if [[ -n "${SUB_TOKEN}" ]]; then
    echo "  token: set (append &token=... yourself)"
  else
    echo "  token: not set"
  fi
fi
