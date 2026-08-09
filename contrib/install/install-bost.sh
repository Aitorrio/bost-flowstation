#!/usr/bin/env bash
# Install bost-flowstation on Raspberry Pi OS / Debian arm64.
# Idempotent: safe to re-run. Starts with phy_io.backend=None so the web UI
# comes up without an SDR; complete Setup in the dashboard afterward.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Aitorrio/bost-flowstation/bost/contrib/install/install-bost.sh | sudo bash
#   sudo ./contrib/install/install-bost.sh
#
# Env:
#   BOST_SRC          existing checkout (default: /opt/bost-flowstation)
#   BOST_BRANCH       git branch (default: bost)
#   BOST_REPO         git URL (default: https://github.com/Aitorrio/bost-flowstation.git)
#   BOST_USE_DEB=1    prefer .deb asset if available (optional)
#   BOST_SKIP_BUILD=1 skip cargo build (use existing binary)
set -euo pipefail

REPO_URL="${BOST_REPO:-https://github.com/Aitorrio/bost-flowstation.git}"
BRANCH="${BOST_BRANCH:-bost}"
SRC_ROOT="${BOST_SRC:-/opt/bost-flowstation}"
CFG_DIR="/etc/flowstation"
CFG_PATH="${CFG_DIR}/config.toml"
BIN_PATH="/usr/local/bin/bluestation-bs"
UNIT_NAME="bluestation-bs.service"
HELPER_DST="/usr/local/sbin/bost-setup-helper.sh"
SUDOERS_DST="/etc/sudoers.d/bost-setup"
SERVICE_USER="${BOST_SERVICE_USER:-bts}"

log() { echo "==> $*"; }
warn() { echo "WARNING: $*" >&2; }
die() { echo "ERROR: $*" >&2; exit 1; }

if [[ "$(id -u)" -ne 0 ]]; then
  die "run as root (sudo ./contrib/install/install-bost.sh)"
fi

ARCH="$(uname -m)"
if [[ "$ARCH" != "aarch64" && "$ARCH" != "arm64" ]]; then
  warn "expected aarch64/arm64, found $ARCH — continuing anyway"
fi
if [[ ! -f /etc/debian_version ]]; then
  warn "non-Debian host — apt steps may fail"
fi

# Ensure a service user exists for optional non-root runs / home paths.
if ! id "$SERVICE_USER" >/dev/null 2>&1; then
  log "Creating user $SERVICE_USER"
  useradd -m -s /bin/bash "$SERVICE_USER" || true
fi

export DEBIAN_FRONTEND=noninteractive
log "Installing apt dependencies"
apt-get update -qq
apt-get install -y \
  build-essential pkg-config git curl ca-certificates python3 \
  libssl-dev cmake \
  soapysdr-tools libsoapysdr-dev \
  || apt-get install -y build-essential git curl ca-certificates python3 soapysdr-tools libsoapysdr-dev

# Lime packages when present in the distro (optional at install time).
for p in soapysdr-module-lms7 soapysdr0.8-module-lms7; do
  if apt-cache show "$p" >/dev/null 2>&1; then
    apt-get install -y "$p" || true
  fi
done

# Rust toolchain for the service user (or root if building as root).
install_rust() {
  local user="$1"
  local home
  home="$(getent passwd "$user" | cut -d: -f6)"
  if [[ -x "$home/.cargo/bin/cargo" ]]; then
    return 0
  fi
  log "Installing rustup for $user"
  sudo -u "$user" bash -lc 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable'
}

# Source tree
if [[ -n "${BOST_SRC:-}" && -d "$BOST_SRC/.git" ]]; then
  SRC_ROOT="$BOST_SRC"
  log "Using existing source at $SRC_ROOT"
elif [[ -d "$SRC_ROOT/.git" ]]; then
  log "Updating $SRC_ROOT ($BRANCH)"
  git -C "$SRC_ROOT" fetch --depth 1 origin "$BRANCH" || git -C "$SRC_ROOT" fetch origin "$BRANCH"
  git -C "$SRC_ROOT" checkout "$BRANCH"
  git -C "$SRC_ROOT" pull --ff-only origin "$BRANCH" || true
else
  log "Cloning $REPO_URL ($BRANCH) → $SRC_ROOT"
  mkdir -p "$(dirname "$SRC_ROOT")"
  git clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$SRC_ROOT"
fi

chown -R "$SERVICE_USER:$SERVICE_USER" "$SRC_ROOT" || true

BUILT_BIN=""
if [[ "${BOST_USE_DEB:-0}" == "1" ]]; then
  warn "BOST_USE_DEB=1 requested but .deb auto-download is not wired yet; building from source"
fi

if [[ "${BOST_SKIP_BUILD:-0}" != "1" ]]; then
  install_rust "$SERVICE_USER"
  log "Building bluestation-bs (release) — this can take a while on a Pi"
  sudo -u "$SERVICE_USER" bash -lc "source \"\$HOME/.cargo/env\" && cd \"$SRC_ROOT\" && cargo build --release -p bluestation-bs"
  BUILT_BIN="$SRC_ROOT/target/release/bluestation-bs"
else
  BUILT_BIN="$SRC_ROOT/target/release/bluestation-bs"
  [[ -x "$BUILT_BIN" ]] || die "BOST_SKIP_BUILD=1 but binary missing at $BUILT_BIN"
fi

install -m 755 "$BUILT_BIN" "$BIN_PATH"
log "Installed $BIN_PATH"

# Initial config (only if missing)
mkdir -p "$CFG_DIR"
if [[ ! -f "$CFG_PATH" ]]; then
  log "Writing initial $CFG_PATH (backend=None, dashboard admin/1234)"
  if [[ -f "$SRC_ROOT/example_config/config.toml" ]]; then
    cp "$SRC_ROOT/example_config/config.toml" "$CFG_PATH"
    python3 - "$CFG_PATH" <<'PY'
import sys, re
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
text = re.sub(r'(?m)^backend\s*=\s*".*"', 'backend = "None"', text, count=1)
if re.search(r'(?m)^\s*service_name\s*=', text):
    text = re.sub(r'(?m)^#?\s*service_name\s*=.*', 'service_name = "bluestation-bs"', text, count=1)
elif re.search(r'(?m)^#\s*service_name\s*=', text):
    text = re.sub(r'(?m)^#\s*service_name\s*=.*', 'service_name = "bluestation-bs"', text, count=1)
else:
    text = 'service_name = "bluestation-bs"\n' + text
dash = '\n[dashboard]\nbind = "0.0.0.0"\nport = 8080\nusername = "admin"\npassword = "1234"\nsource_dir = "/opt/bost-flowstation"\n'
if "[dashboard]" not in text:
    text += dash
else:
    block = text.split("[dashboard]", 1)[1].split("\n[")[0]
    if "username" not in block:
        text = text.replace("[dashboard]", "[dashboard]" + dash.replace("[dashboard]", ""), 1)
open(path, "w", encoding="utf-8").write(text)
PY
  else
    cat >"$CFG_PATH" <<'EOF'
# bost-flowstation first-boot config — complete Setup in the web UI
config_version = "0.6"
stack_mode = "Bs"
service_name = "bluestation-bs"

[phy_io]
backend = "None"

[phy_io.soapysdr]
tx_freq = 438025000.0
rx_freq = 433025000.0

[net_info]
mcc = 204
mnc = 1337

[cell_info]
freq_band = 4
main_carrier = 1521
duplex_spacing = 4
freq_offset = 0
reverse_operation = false
location_area = 2
colour_code = 1
local_ssi_ranges = [[0, 90]]
system_wide_services = true
voice_service = true

[dashboard]
bind = "0.0.0.0"
port = 8080
username = "admin"
password = "1234"
source_dir = "/opt/bost-flowstation"
EOF
  fi
  cp "$CFG_PATH" "${CFG_PATH}.fallback"
else
  log "Keeping existing $CFG_PATH"
fi

if [[ ! -f "${CFG_DIR}/setup.json" ]]; then
  cat >"${CFG_DIR}/setup.json" <<'EOF'
{
  "setup_complete": false,
  "skipped": false,
  "version": 1
}
EOF
fi

# Helper + sudoers
install -m 755 "$SRC_ROOT/contrib/install/bost-setup-helper.sh" "$HELPER_DST"
cat >"$SUDOERS_DST" <<EOF
# Allow the FlowStation service user to run only the bost setup helper (no free shell).
${SERVICE_USER} ALL=(root) NOPASSWD: ${HELPER_DST}
Defaults!${HELPER_DST} !requiretty
EOF
chmod 440 "$SUDOERS_DST"
visudo -cf "$SUDOERS_DST" >/dev/null || die "invalid sudoers drop-in"

# systemd unit
UNIT_DST="/etc/systemd/system/${UNIT_NAME}"
cat >"$UNIT_DST" <<EOF
[Unit]
Description=bost-flowstation TETRA base station (bluestation-bs)
Documentation=https://github.com/Aitorrio/bost-flowstation
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=300
StartLimitBurst=10

[Service]
Type=simple
User=root
Environment=BOST_SETUP_HELPER=${HELPER_DST}
Environment=BOST_SERVICE_UNIT=${UNIT_NAME}
Environment=BOST_SRC=${SRC_ROOT}
ExecStart=${BIN_PATH} ${CFG_PATH}
# Real-time scheduling helps the PHY loop when RF is enabled.
CPUSchedulingPolicy=fifo
CPUSchedulingPriority=73
KillSignal=SIGINT
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now "$UNIT_NAME"

# Primary LAN IP hint
IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
IP="${IP:-<pi-ip>}"

echo
echo "────────────────────────────────────────────────────────"
echo " bost-flowstation installed"
echo " Dashboard:  http://${IP}:8080"
echo " Login:      admin / 1234"
echo " Config:     ${CFG_PATH}"
echo " Setup:      open the Setup tab / first-run wizard"
echo " RF starts disabled (backend=None) until you finish Setup."
echo "────────────────────────────────────────────────────────"
