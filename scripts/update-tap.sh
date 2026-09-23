#!/usr/bin/env bash
# Regenerate Formula/statefulmemory.rb in the Homebrew tap for a released tag.
#
# Usage: scripts/update-tap.sh v0.1.0 /path/to/homebrew-tap
#
# Fetches SHA256SUMS from the GitHub release and writes
# <tap>/Formula/statefulmemory.rb. Run after every release tag, then commit
# and push the tap repo so `brew upgrade` picks it up.
set -euo pipefail

TAG="${1:?usage: update-tap.sh <tag> <tap-repo-dir>}"
TAP_DIR="${2:?usage: update-tap.sh <tag> <tap-repo-dir>}"
REPO="${STATEFULMEMORY_RELEASE_REPO:-sanudatta11/statefulmemory}"
SUMS_URL="https://github.com/${REPO}/releases/download/${TAG}/SHA256SUMS"
echo "Fetching ${SUMS_URL}"
SUMS="$(curl -fsSL "${SUMS_URL}")"

sha_for() {
  local asset="$1"
  local line
  line="$(printf '%s\n' "${SUMS}" | awk -v f="${asset}" '$2 == f { print $1 }')"
  if [[ -z "${line}" ]]; then
    echo "error: no sha256 for ${asset} in SHA256SUMS" >&2
    exit 1
  fi
  printf '%s' "${line}"
}

SHA_DARWIN_ARM64="$(sha_for "statefulmemory-${TAG}-darwin-arm64.tar.gz")"
SHA_DARWIN_AMD64="$(sha_for "statefulmemory-${TAG}-darwin-amd64.tar.gz")"
SHA_LINUX_ARM64="$(sha_for "statefulmemory-${TAG}-linux-arm64.tar.gz")"
SHA_LINUX_AMD64="$(sha_for "statefulmemory-${TAG}-linux-amd64.tar.gz")"

FORMULA_DIR="${TAP_DIR}/Formula"
mkdir -p "${FORMULA_DIR}"
FORMULA="${FORMULA_DIR}/statefulmemory.rb"

cat > "${FORMULA}" <<EOF
class Statefulmemory < Formula
  desc "Persistent memory for AI coding agents (SQLite + hybrid search + MCP)"
  homepage "https://statefulmemory.dev"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/${REPO}/releases/download/${TAG}/statefulmemory-${TAG}-darwin-arm64.tar.gz"
      sha256 "${SHA_DARWIN_ARM64}"
    end
    on_intel do
      url "https://github.com/${REPO}/releases/download/${TAG}/statefulmemory-${TAG}-darwin-amd64.tar.gz"
      sha256 "${SHA_DARWIN_AMD64}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/${REPO}/releases/download/${TAG}/statefulmemory-${TAG}-linux-arm64.tar.gz"
      sha256 "${SHA_LINUX_ARM64}"
    end
    on_intel do
      url "https://github.com/${REPO}/releases/download/${TAG}/statefulmemory-${TAG}-linux-amd64.tar.gz"
      sha256 "${SHA_LINUX_AMD64}"
    end
  end

  def install
    # One build, three names (Makefile parity): statefulmemory, smem, sm.
    bin.install "statefulmemory", "smem", "sm"
  end

  def caveats
    <<~EOS
      Wire agents, git hooks, and (optionally) the Laya System-1 sidecar:

        statefulmemory install

      Data dir:  ~/.statefulmemory/
      First hybrid-search run downloads the BGE model (~120 MB) into
      ~/.statefulmemory-models/ .
      Python 3.10+ recommended for Laya (skip with: statefulmemory install --no-laya).
      The daemon auto-starts on first use.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/statefulmemory --version")
    assert_match version.to_s, shell_output("#{bin}/smem --version")
    assert_match version.to_s, shell_output("#{bin}/sm --version")
    # \`version\` is daemon-free (direct print, no spawn).
    assert_match version.to_s, shell_output("#{bin}/statefulmemory version")
  end
end
EOF

echo "Wrote ${FORMULA}"
