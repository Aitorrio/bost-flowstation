#!/usr/bin/env bash
# Build and install outerplane tetra-codec (libtetra-codec.so + pkg-config) for LST voice / optional Asterisk SIP.
# Idempotent: skips work when `pkg-config --exists tetra-codec` already succeeds.
#
# Usage (as root):
#   sudo ./contrib/install/install-tetra-codec.sh
#   sudo BOST_TETRA_CODEC_FORCE=1 ./contrib/install/install-tetra-codec.sh   # rebuild even if present
#
# Env:
#   BOST_TETRA_CODEC_SRC   clone/build dir (default: /opt/bost-deps/tetra-codec)
#   BOST_TETRA_CODEC_GIT   git URL (default: https://github.com/outerplane/tetra-codec.git)
#   BOST_TETRA_CODEC_REF   branch/tag (default: master)
#   BOST_TETRA_CODEC_PREFIX install prefix (default: /usr/local)
#   BOST_TETRA_CODEC_FORCE=1  rebuild/reinstall even if pkg-config already finds it
#   BOST_SKIP_TETRA_CODEC=1   no-op success (for air-gapped / signalling-only installs)
set -euo pipefail

SRC="${BOST_TETRA_CODEC_SRC:-/opt/bost-deps/tetra-codec}"
GIT_URL="${BOST_TETRA_CODEC_GIT:-https://github.com/outerplane/tetra-codec.git}"
GIT_REF="${BOST_TETRA_CODEC_REF:-master}"
PREFIX="${BOST_TETRA_CODEC_PREFIX:-/usr/local}"
JOBS="${BOST_TETRA_CODEC_JOBS:-$(nproc 2>/dev/null || echo 1)}"

log() { echo "==> tetra-codec: $*"; }
warn() { echo "WARNING: tetra-codec: $*" >&2; }
die() { echo "ERROR: tetra-codec: $*" >&2; exit 1; }

if [[ "${BOST_SKIP_TETRA_CODEC:-0}" == "1" ]]; then
  log "BOST_SKIP_TETRA_CODEC=1 — skipping"
  exit 0
fi

if [[ "$(id -u)" -ne 0 ]]; then
  die "run as root (sudo $0)"
fi

codec_present() {
  if pkg-config --exists tetra-codec 2>/dev/null; then
    return 0
  fi
  # pkg-config may miss a just-installed .pc until PKG_CONFIG_PATH is set.
  local so
  for so in \
    "${PREFIX}/lib/libtetra-codec.so" \
    "${PREFIX}/lib/aarch64-linux-gnu/libtetra-codec.so" \
    "${PREFIX}/lib/arm-linux-gnueabihf/libtetra-codec.so" \
    /usr/local/lib/libtetra-codec.so \
    /usr/lib/libtetra-codec.so; do
    [[ -f "$so" ]] && return 0
  done
  return 1
}

export PKG_CONFIG_PATH="${PREFIX}/lib/pkgconfig:${PREFIX}/lib/aarch64-linux-gnu/pkgconfig:${PKG_CONFIG_PATH:-}"

if codec_present && [[ "${BOST_TETRA_CODEC_FORCE:-0}" != "1" ]]; then
  log "already installed ($(pkg-config --modversion tetra-codec 2>/dev/null || echo ok)) — skip"
  pkg-config --libs tetra-codec 2>/dev/null || true
  exit 0
fi

export DEBIAN_FRONTEND=noninteractive
if command -v apt-get >/dev/null 2>&1; then
  log "ensuring cmake / build tools"
  apt-get update -qq
  apt-get install -y --no-install-recommends \
    build-essential cmake pkg-config git ca-certificates \
    || apt-get install -y build-essential cmake pkg-config git
fi

command -v cmake >/dev/null 2>&1 || die "cmake not found"
command -v git >/dev/null 2>&1 || die "git not found"

mkdir -p "$(dirname "$SRC")"
if [[ -d "$SRC/.git" ]]; then
  log "updating $SRC"
  git -C "$SRC" fetch --depth 1 origin "$GIT_REF" || git -C "$SRC" fetch origin "$GIT_REF" || true
  git -C "$SRC" checkout -f "$GIT_REF" 2>/dev/null \
    || git -C "$SRC" checkout -f "origin/$GIT_REF" 2>/dev/null \
    || git -C "$SRC" pull --ff-only || true
else
  log "cloning $GIT_URL ($GIT_REF) → $SRC"
  rm -rf "$SRC"
  git clone --depth 1 --branch "$GIT_REF" "$GIT_URL" "$SRC" \
    || git clone --depth 1 "$GIT_URL" "$SRC"
fi

[[ -f "$SRC/CMakeLists.txt" ]] || die "CMakeLists.txt missing in $SRC"

log "configuring (prefix=$PREFIX)"
cmake -S "$SRC" -B "$SRC/build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DTETRA_CODEC_LIBRARY_TYPE=SHARED

log "building (-j $JOBS)"
cmake --build "$SRC/build" -j"$JOBS"

log "installing to $PREFIX"
cmake --install "$SRC/build"
ldconfig || true

export PKG_CONFIG_PATH="${PREFIX}/lib/pkgconfig:${PREFIX}/lib/aarch64-linux-gnu/pkgconfig:${PKG_CONFIG_PATH:-}"
if ! codec_present; then
  die "install finished but libtetra-codec still not visible (check $PREFIX/lib and PKG_CONFIG_PATH)"
fi

log "OK — $(pkg-config --libs tetra-codec 2>/dev/null || echo libtetra-codec installed)"
