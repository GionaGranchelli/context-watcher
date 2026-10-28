#!/bin/sh
# contextWatcher (cw) — Universal Context & KV-Cache Optimizer
# One-line installer: curl -fsSL https://constant-labs.com/cw-install.sh | sh
#
# Copyright (c) 2026 Giona Granchelli
# Licensed under MIT OR Apache-2.0

set -e

# Repository configuration
REPO="GionaGranchelli/context-watcher"
BIN_NAME="cw"
INSTALL_DIR="${CW_INSTALL_DIR:-$HOME/.local/bin}"

# Text formatting
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    BOLD="\033[1m"
    DIM="\033[2m"
    GREEN="\033[32m"
    BLUE="\033[34m"
    CYAN="\033[36m"
    YELLOW="\033[33m"
    RED="\033[31m"
    RESET="\033[0m"
else
    BOLD=""
    DIM=""
    GREEN=""
    BLUE=""
    CYAN=""
    YELLOW=""
    RED=""
    RESET=""
fi

info() {
    printf "${CYAN}==>${RESET} ${BOLD}%s${RESET}\n" "$1"
}

success() {
    printf "${GREEN}==>${RESET} ${BOLD}%s${RESET}\n" "$1"
}

warn() {
    printf "${YELLOW}warning:${RESET} %s\n" "$1"
}

error() {
    printf "${RED}error:${RESET} %b\n" "$1" >&2
    exit 1
}

# Temporary directory handling
TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'cw-install')"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

# Detect Operating System & Architecture
detect_platform() {
    OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
    ARCH="$(uname -m)"

    case "$OS" in
        linux*)
            TARGET_OS="unknown-linux-gnu"
            ;;
        darwin*)
            TARGET_OS="apple-darwin"
            ;;
        *)
            error "Unsupported operating system: $OS. contextWatcher supports Linux and macOS."
            ;;
    esac

    case "$ARCH" in
        x86_64|amd64)
            TARGET_ARCH="x86_64"
            ;;
        aarch64|arm64)
            TARGET_ARCH="aarch64"
            ;;
        *)
            error "Unsupported architecture: $ARCH. contextWatcher supports x86_64 and aarch64."
            ;;
    esac

    TARGET="${TARGET_ARCH}-${TARGET_OS}"
    ARTIFACT_NAME="cw-${TARGET}.tar.gz"
}

# Banner
print_banner() {
    printf "${CYAN}"
    cat << 'EOF'
               _            _ __          __   _       _               
  ___ ___  _ _| |_ _____ __| |\ \        / /__| |_ ___| |_  ___ _ _ 
 / _/ _ \| ' \  _/ -_) \ /  _| \ \  /\  / / _ \  _/ _ \ ' \/ -_) '_|
 \__\___/|_||_\__\___/_\_\___|  \_\/  \/ \___/\__\___/_||_\___|_|  
EOF
    printf "${RESET}"
    printf "${BOLD}contextWatcher (cw)${RESET} — Ultra-Low Latency LLM Context & KV-Cache Optimizer\n"
    printf "${DIM}Dual-licensed under MIT / Apache-2.0 • Constant Labs (constant-labs.com)${RESET}\n\n"
}

# Install via pre-compiled release tarball
install_from_release() {
    DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ARTIFACT_NAME}"
    info "Fetching pre-built binary for ${TARGET}..."

    DOWNLOAD_FILE="${TMP_DIR}/${ARTIFACT_NAME}"
    HTTP_CODE=0

    if command -v curl >/dev/null 2>&1; then
        HTTP_CODE=$(curl -sSL -w "%{http_code}" -o "$DOWNLOAD_FILE" "$DOWNLOAD_URL" || true)
    elif command -v wget >/dev/null 2>&1; then
        if wget -q -O "$DOWNLOAD_FILE" "$DOWNLOAD_URL"; then
            HTTP_CODE=200
        else
            HTTP_CODE=404
        fi
    else
        error "Neither curl nor wget found. Please install curl or wget."
    fi

    if [ "$HTTP_CODE" -eq 200 ] && [ -s "$DOWNLOAD_FILE" ]; then
        info "Extracting ${ARTIFACT_NAME}..."
        tar -xzf "$DOWNLOAD_FILE" -C "$TMP_DIR"
        mkdir -p "$INSTALL_DIR"
        cp "$TMP_DIR/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
        chmod +x "$INSTALL_DIR/$BIN_NAME"
        return 0
    fi

    return 1
}

# Fallback: build from source via cargo
install_from_source() {
    warn "Pre-built binary for ${TARGET} not found on GitHub Releases."
    if command -v cargo >/dev/null 2>&1; then
        info "Rust toolchain detected. Compiling contextWatcher from source via cargo..."
        cargo install --git "https://github.com/${REPO}.git" cw-cli --root "$(dirname "${INSTALL_DIR}")"
        mkdir -p "$INSTALL_DIR"
        # cargo --root "$(dirname "$INSTALL_DIR")" already places the binary at
        # "$INSTALL_DIR/$BIN_NAME"; only copy when it landed elsewhere (custom CW_INSTALL_DIR).
        if [ -x "${INSTALL_DIR}/../bin/$BIN_NAME" ] && [ ! -x "${INSTALL_DIR}/$BIN_NAME" ]; then
            cp "${INSTALL_DIR}/../bin/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
            chmod +x "$INSTALL_DIR/$BIN_NAME"
        fi
        [ -x "$INSTALL_DIR/$BIN_NAME" ] || error "cargo install finished but $INSTALL_DIR/$BIN_NAME is missing."
        return 0
    else
        error "Pre-built binary unavailable and cargo is not installed.\nTo install Rust, visit: https://rustup.rs or run:\n  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    fi
}

# Configure PATH
ensure_path() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*)
            ;;
        *)
            printf "\n"
            warn "$INSTALL_DIR is not in your \$PATH."
            SHELL_NAME="$(basename "${SHELL:-bash}")"
            PROFILE_FILE=""
            case "$SHELL_NAME" in
                zsh)
                    PROFILE_FILE="$HOME/.zshrc"
                    ;;
                bash)
                    if [ -f "$HOME/.bashrc" ]; then
                        PROFILE_FILE="$HOME/.bashrc"
                    else
                        PROFILE_FILE="$HOME/.bash_profile"
                    fi
                    ;;
                fish)
                    PROFILE_FILE="$HOME/.config/fish/config.fish"
                    ;;
                *)
                    PROFILE_FILE="$HOME/.profile"
                    ;;
            esac

            EXPORT_LINE="export PATH=\"\$PATH:$INSTALL_DIR\""
            if [ "$SHELL_NAME" = "fish" ]; then
                EXPORT_LINE="set -gx PATH \$PATH $INSTALL_DIR"
            fi

            if [ -n "$PROFILE_FILE" ] && [ -f "$PROFILE_FILE" ]; then
                printf "\n# contextWatcher\n%s\n" "$EXPORT_LINE" >> "$PROFILE_FILE"
                success "Added $INSTALL_DIR to $PROFILE_FILE"
                warn "Run 'source $PROFILE_FILE' or start a new terminal session."
            else
                info "Please add the following line to your shell configuration:"
                printf "  %s\n\n" "$EXPORT_LINE"
            fi
            ;;
    esac
}

# Main installation flow
main() {
    print_banner
    detect_platform

    info "Installing contextWatcher to ${INSTALL_DIR}..."

    if ! install_from_release; then
        install_from_source
    fi

    # Verify binary
    if [ ! -x "$INSTALL_DIR/$BIN_NAME" ]; then
        error "Installation failed: $INSTALL_DIR/$BIN_NAME is not executable."
    fi

    VERSION="$("$INSTALL_DIR/$BIN_NAME" --version 2>/dev/null || echo "cw")"
    success "Successfully installed ${VERSION}!"

    # Ensure PATH
    ensure_path

    # Run auto-initialization for agent harnesses
    printf "\n"
    info "Running 'cw init' to register agent harness integrations..."
    # Do not enable an always-on background service from a piped installer; let the
    # user opt in explicitly.
    "$INSTALL_DIR/$BIN_NAME" init --no-start || true

    printf "\n"
    success "Installation and setup complete!"
    printf "\n"
    printf "  ${BOLD}Next steps:${RESET}\n"
    printf "    ${CYAN}cw daemon start${RESET}   Start the background daemon (Web UI :4400, proxy :4401)\n"
    printf "    ${DIM}systemctl --user enable --now contextwatcher${RESET}  (optional) start it at every login\n"
    printf "    ${CYAN}cw gain${RESET}        Show token, KV cache & dollar savings scoreboard\n"
    printf "    ${CYAN}cw hud${RESET}         Display instant sub-millisecond shell statusline\n"
    printf "    ${CYAN}cw pulse${RESET}       View recent ambient background savings notifications\n"
    printf "    ${CYAN}cw status${RESET}      Inspect active session context & VRAM footprint\n"
    printf "    ${CYAN}cw --help${RESET}      Explore all contextWatcher commands\n\n"
}

main "$@"
