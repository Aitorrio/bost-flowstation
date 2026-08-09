#!/usr/bin/env bash
# Allowlisted privileged helper for the FlowStation / bost-flowstation Setup wizard.
# Invoked only as: sudo -n /usr/local/sbin/bost-setup-helper.sh <action> [args]
# Never accept free-form shell from the dashboard.
set -euo pipefail

ACTION="${1:-}"
shift || true

SERVICE_UNIT="${BOST_SERVICE_UNIT:-bluestation-bs.service}"
SRC_ROOT="${BOST_SRC:-/opt/bost-flowstation}"
SOAPY_SX_DIR="${BOST_SOAPY_SX_DIR:-${HOME:-/root}/sxxcvr}"

log() { echo "[bost-setup-helper] $*"; }

die() { echo "[bost-setup-helper] ERROR: $*" >&2; exit 1; }

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    die "must run as root (via sudoers drop-in)"
  fi
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
  # Prefer an existing SoapySX / sxxcvr tree (common on this fork's Pis).
  local candidates=(
    "$SOAPY_SX_DIR"
    /home/bts/sxxcvr
    /opt/sxxcvr
    /usr/local/src/SoapySX
  )
  local found=""
  for d in "${candidates[@]}"; do
    if [[ -d "$d" ]]; then
      found="$d"
      break
    fi
  done

  if [[ -z "$found" ]]; then
    log "Cloning SoapySX into $SOAPY_SX_DIR"
    mkdir -p "$(dirname "$SOAPY_SX_DIR")"
    if ! command -v git >/dev/null; then
      apt-get update -qq
      apt-get install -y git cmake build-essential
    fi
    git clone --depth 1 https://github.com/pothosware/SoapySDR.git /tmp/SoapySDR-src 2>/dev/null || true
    # SXceiver vendor module — try well-known mirrors; operator can set BOST_SOAPY_SX_GIT
    local git_url="${BOST_SOAPY_SX_GIT:-}"
    if [[ -z "$git_url" ]]; then
      die "SXceiver tree not found. Set BOST_SOAPY_SX_DIR to an existing SoapySX build, or BOST_SOAPY_SX_GIT to clone URL"
    fi
    git clone --depth 1 "$git_url" "$SOAPY_SX_DIR"
    found="$SOAPY_SX_DIR"
  fi

  log "Building SoapySX in $found"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y cmake build-essential libsoapysdr-dev soapysdr-tools || true
  if [[ -f "$found/CMakeLists.txt" ]]; then
    cmake -S "$found" -B "$found/build" -DCMAKE_BUILD_TYPE=Release
    cmake --build "$found/build" -j"$(nproc)"
    cmake --install "$found/build"
    ldconfig || true
  elif [[ -x "$found/install.sh" ]]; then
    (cd "$found" && bash ./install.sh)
  else
    die "no CMakeLists.txt or install.sh in $found"
  fi
  log "SXceiver driver install finished"
  SoapySDRUtil --find 2>/dev/null | head -n 40 || true
}

enable_service() {
  systemctl enable --now "$SERVICE_UNIT"
  systemctl is-enabled "$SERVICE_UNIT"
  systemctl is-active "$SERVICE_UNIT" || true
  log "enabled $SERVICE_UNIT"
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
