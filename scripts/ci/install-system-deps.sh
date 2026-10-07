#!/usr/bin/env bash
# Ensure the system packages a CI job needs, without assuming `apt-get update`
# is cheap. Extra packages may be passed as arguments.
#
#   scripts/ci/install-system-deps.sh [pkg ...]
#
# Why this exists instead of a bare `sudo apt-get update && apt-get install`:
#
#   * The GitHub `ubuntu-*` images list BOTH `archive.ubuntu.com` and
#     `azure.archive.ubuntu.com` in their apt sources. When the azure mirror has
#     no route from the runner, `apt-get update` retries it for ~20 minutes
#     before falling through to the mirror that works. That alone burned a
#     20-minute job budget on 2026-10-07 and killed the round-trip gate before
#     the build ever started.
#   * The images already ship nearly everything we need, so the common case
#     should not touch the network at all.
#
# So: detect what is missing, and only touch apt when something is genuinely
# absent — after removing the dead mirror so the slow path is fast too.
#
# Packages the Rust workspace build needs, and why:
#   build-essential    cc/linker for the -sys crates (sqlite-vec, aws-lc-sys,
#                      ring) and for `cc`-driven builds.
#   pkg-config         locate libssl for openssl-sys.
#   libssl-dev         OpenSSL headers for openssl-sys, reached via
#                      ureq -> native-tls (the workspace uses default-features).
#   protobuf-compiler  `protoc`, required by statefulmemory-proto/build.rs.

set -euo pipefail

# --- detection -------------------------------------------------------------

# True when dpkg has the package installed.
pkg_installed() {
  [ "$(dpkg-query -W -f='${Status}' "$1" 2>/dev/null || true)" = "install ok installed" ]
}

missing=()

command -v protoc >/dev/null 2>&1 || missing+=(protobuf-compiler)
command -v cc >/dev/null 2>&1 || missing+=(build-essential)
command -v pkg-config >/dev/null 2>&1 || missing+=(pkg-config)

# openssl-sys needs the headers, not merely the shared library. `pkg-config
# --exists` covers the usual layout; the header check is the fallback for
# installs that ship headers without an openssl.pc.
if ! pkg-config --exists openssl 2>/dev/null && [ ! -e /usr/include/openssl/ssl.h ]; then
  missing+=(libssl-dev)
fi

# Caller-supplied extras (e.g. `cmake` for the release build, `ffmpeg` for the
# video render). Deduped so an extra that is already in the list above is not
# requested twice.
for extra in "$@"; do
  # An unset CI variable expanding to "" is not an error worth failing a build
  # over, so skip it rather than treat it as a package name.
  [ -z "$extra" ] && continue
  case "$extra" in
    *[!a-zA-Z0-9.+-]*)
      echo "install-system-deps: refusing suspicious package name: '$extra'" >&2
      exit 2
      ;;
  esac
  pkg_installed "$extra" || missing+=("$extra")
done

if [ "${#missing[@]}" -gt 0 ]; then
  # Strip duplicates, preserving order.
  deduped=()
  for pkg in "${missing[@]}"; do
    seen=0
    for kept in "${deduped[@]:-}"; do
      [ "$kept" = "$pkg" ] && seen=1 && break
    done
    [ "$seen" -eq 0 ] && deduped+=("$pkg")
  done
  missing=("${deduped[@]:-}")
fi

if [ "${#missing[@]}" -eq 0 ]; then
  echo "system deps already present; skipping apt (no network round-trip)"
  command -v protoc >/dev/null 2>&1 && protoc --version
  exit 0
fi

echo "missing system deps: ${missing[*]}"

# --- install ---------------------------------------------------------------

# Drop the unreachable mirror before updating. Handles both the legacy one-line
# format (/etc/apt/sources.list) and the deb822 format
# (/etc/apt/sources.list.d/ubuntu.sources, which uses `URIs:`), since matching
# on the hostname catches the mirror in either.
sudo sed -i '/azure\.archive\.ubuntu\.com/d' \
  /etc/apt/sources.list \
  /etc/apt/sources.list.d/*.list \
  /etc/apt/sources.list.d/*.sources 2>/dev/null || true

# Bound the damage if some other mirror is also dead: a couple of retries, a
# per-request timeout, and a hard ceiling on the whole update.
sudo DEBIAN_FRONTEND=noninteractive apt-get \
  -o Acquire::Retries=2 \
  -o Acquire::http::Timeout=30 \
  -o Acquire::https::Timeout=30 \
  update

sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  "${missing[@]}"

# --- verify ----------------------------------------------------------------

if command -v protoc >/dev/null 2>&1; then
  protoc --version
fi
pkg-config --modversion openssl
