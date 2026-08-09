#!/usr/bin/env bash
# Allowlisted privileged helper for the FlowStation / bost-flowstation Setup wizard.
# Invoked only as: sudo -n /usr/local/sbin/bost-setup-helper.sh <action> [args]
# Never accept free-form shell from the dashboard.
set -euo pipefail

ACTION="${1:-}"
shift || true

SERVICE_UNIT="${BOST_SERVICE_UNIT:-bluestation-bs.service}"
SRC_ROOT="${BOST_SRC:-/opt/bost-flowstation}"
# Official SXceiver software tree (SoapySX lives in sxxcvr/SoapySX).
SOAPY_SX_GIT="${BOST_SOAPY_SX_GIT:-https://github.com/tejeez/sxxcvr.git}"
SOAPY_SX_DIR="${BOST_SOAPY_SX_DIR:-/opt/sxxcvr}"

log() { echo "[bost-setup-helper] $*"; }

die() { echo "[bost-setup-helper] ERROR: $*" >&2; exit 1; }

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    die "must run as root (via sudoers drop-in)"
  fi
}

# Resolve a directory that contains SoapySX/CMakeLists.txt (or is SoapySX itself).
resolve_soapysx_src() {
  local root="$1"
  if [[ -f "$root/SoapySX/CMakeLists.txt" ]]; then
    echo "$root/SoapySX"
    return 0
  fi
  if [[ -f "$root/CMakeLists.txt" ]]; then
    echo "$root"
    return 0
  fi
  return 1
}

install_driver_lime() {
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  # Package names vary across Debian/RPi OS releases.
  local pkgs=()
  for p in soapysdr-module-lms7 soapysdr0.8-module-lms7 soapysdr0.7-module-lms7 limesuite; do
    if apt-cache show "$p" >/dev/null 2>&1; then
      pkgs+=("$p")
    fi
  done
  if [[ ${#pkgs[@]} -eq 0 ]]; then
    die "no LimeSDR Soapy packages found in apt"
  fi
  apt-get install -y "${pkgs[@]}"
  log "LimeSDR modules installed: ${pkgs[*]}"
  SoapySDRUtil --info 2>/dev/null | head -n 20 || true
}

install_driver_sx() {
  # Official flow from https://sxceiver.com/doc/getting-started and tejeez/sxxcvr.
  local candidates=(
    "$SOAPY_SX_DIR"
    /home/bts/sxxcvr
    /opt/sxxcvr
    /usr/local/src/sxxcvr
    /usr/local/src/SoapySX
  )
  local tree=""
  for d in "${candidates[@]}"; do
    if [[ -d "$d" ]] && resolve_soapysx_src "$d" >/dev/null; then
      tree="$d"
      break
    fi
  done

  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y --no-install-recommends \
    git make g++ cmake \
    libsoapysdr-dev libasound2-dev soapysdr-tools python3-soapysdr \
    || apt-get install -y git make g++ cmake libsoapysdr-dev libasound2-dev soapysdr-tools

  # Optional on some boards / Debian releases.
  apt-get install -y libgpiod-dev 2>/dev/null || true

  if [[ -z "$tree" ]]; then
    log "Cloning SXceiver software from $SOAPY_SX_GIT → $SOAPY_SX_DIR"
    mkdir -p "$(dirname "$SOAPY_SX_DIR")"
    rm -rf "$SOAPY_SX_DIR"
    git clone --depth 1 "$SOAPY_SX_GIT" "$SOAPY_SX_DIR"
    tree="$SOAPY_SX_DIR"
  else
    log "Using existing SXceiver tree at $tree"
    if [[ -d "$tree/.git" ]]; then
      git -C "$tree" pull --ff-only || true
    fi
  fi

  local src
  src="$(resolve_soapysx_src "$tree")" || die "SoapySX/CMakeLists.txt not found under $tree"

  log "Building SoapySX in $src"
  cmake -S "$src" -B "$src/build" -DCMAKE_BUILD_TYPE=Release
  cmake --build "$src/build" -j"$(nproc)"
  cmake --install "$src/build"
  ldconfig || true

  log "SXceiver driver install finished"
  SoapySDRUtil --info 2>/dev/null | head -n 30 || true
  echo "---"
  SoapySDRUtil --find 2>/dev/null | head -n 40 || true
  echo "---"
  SoapySDRUtil --probe="driver=sx" 2>&1 | head -n 40 || true
}

enable_service() {
  # Idempotent: never restart a live station just to flip the enable bit.
  # (`enable --now` can disrupt an already-running unit mid-wizard.)
  systemctl enable "$SERVICE_UNIT"
  local enabled active
  enabled="$(systemctl is-enabled "$SERVICE_UNIT" 2>/dev/null || true)"
  active="$(systemctl is-active "$SERVICE_UNIT" 2>/dev/null || true)"
  echo "enabled=${enabled}"
  echo "active=${active}"
  if [[ "$enabled" != "enabled" ]]; then
    die "unit $SERVICE_UNIT is not enabled (got: ${enabled:-unknown})"
  fi
  log "autostart OK for $SERVICE_UNIT (enabled=${enabled}, active=${active})"
}

restart_service() {
  systemctl restart "$SERVICE_UNIT"
  log "restarted $SERVICE_UNIT"
}

require_root

case "$ACTION" in
  install-driver)
    driver="${1:-}"
    case "$driver" in
      sx) install_driver_sx ;;
      lime) install_driver_lime ;;
      *) die "unknown driver '$driver' (sx|lime)" ;;
    esac
    ;;
  enable-service)
    enable_service
    ;;
  restart-service)
    restart_service
    ;;
  *)
    die "unknown action '$ACTION' (install-driver|enable-service|restart-service)"
    ;;
esac
