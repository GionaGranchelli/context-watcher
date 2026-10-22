#!/usr/bin/env bash
# contextWatcher (`cw`) — One-line installer script
# Usage: curl -fsSL https://raw.githubusercontent.com/context-watcher/contextWatcher/main/install.sh | bash

set -euo pipefail

REPO="context-watcher/contextWatcher"
INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "${INSTALL_DIR}"

BOLD="$(tput bold 2>/dev/null || echo '')"
GREEN="$(tput setaf 2 2>/dev/null || echo '')"
CYAN="$(tput setaf 6 2>/dev/null || echo '')"
RESET="$(tput sgr0 2>/dev/null || echo '')"

echo "${BOLD}${CYAN}┌────────────────────────────────────────────────────────────────────────┐${RESET}"
echo "${BOLD}${CYAN}│  contextWatcher (cw) — Universal Context & KV-Cache Optimizer         │${RESET}"
echo "${BOLD}${CYAN}└────────────────────────────────────────────────────────────────────────┘${RESET}"

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "${ARCH}" in
    x86_64|amd64) TARGET_ARCH="x86_64" ;;
    aarch64|arm64) TARGET_ARCH="aarch64" ;;
    *) echo "Unsupported architecture: ${ARCH}"; exit 1 ;;
esac

case "${OS}" in
    linux) TARGET_OS="unknown-linux-gnu" ;;
    darwin) TARGET_OS="apple-darwin" ;;
    *) echo "Unsupported OS: ${OS}"; exit 1 ;;
esac

TARGET="${TARGET_ARCH}-${TARGET_OS}"
echo "Detected platform: ${BOLD}${TARGET}${RESET}"

# Check if prebuilt release exists or fallback to cargo build
DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/cw-${TARGET}.tar.gz"

if command -v curl >/dev/null 2>&1 && curl -s --head "${DOWNLOAD_URL}" | grep "200" >/dev/null 2>&1; then
    echo "Downloading prebuilt release binary from ${DOWNLOAD_URL}..."
    TMP_DIR="$(mktemp -d)"
    curl -fsSL "${DOWNLOAD_URL}" | tar -xz -C "${TMP_DIR}"
    install -m 755 "${TMP_DIR}/cw" "${INSTALL_DIR}/cw"
    rm -rf "${TMP_DIR}"
elif command -v cargo >/dev/null 2>&1; then
    echo "Prebuilt release binary not found; building from crates.io / git with cargo..."
    cargo install --git "https://github.com/${REPO}.git" -p cw-cli --root "${HOME}/.local"
else
    echo "Error: Neither prebuilt binary nor cargo is available."
    exit 1
fi

echo "${GREEN}✓ Successfully installed cw to ${INSTALL_DIR}/cw${RESET}"

# Ensure ~/.local/bin is in PATH
if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
    echo "Notice: Add ${INSTALL_DIR} to your PATH in ~/.bashrc or ~/.zshrc:"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
fi

# Run integrations setup automatically
echo "Configuring integrations for installed agent harnesses..."
"${INSTALL_DIR}/cw" init || true

echo ""
echo "${BOLD}${GREEN}All set! Run 'cw gain' or '/cw optimize mid' in your coding agent.${RESET}"
