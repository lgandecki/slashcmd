#!/bin/sh
# slashcmd installer
# https://slashcmd.lgandecki.net
#
# Inspect this script: curl -sSL slashcmd.lgandecki.net/install.sh
# Build from source:   https://github.com/lgandecki/slashcmd

set -e

REPO="lgandecki/slashcmd"
INSTALL_DIR="${SLASHCMD_INSTALL_DIR:-$HOME/.local/bin}"

# Detect OS and architecture
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin)
            case "$ARCH" in
                arm64) PLATFORM="darwin-arm64" ;;
                x86_64) PLATFORM="darwin-x64" ;;
                *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
            esac
            ;;
        Linux)
            case "$ARCH" in
                x86_64) PLATFORM="linux-x64" ;;
                aarch64) PLATFORM="linux-arm64" ;;
                *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
            esac
            ;;
        *)
            echo "Unsupported OS: $OS"
            echo "Build from source: https://github.com/$REPO"
            exit 1
            ;;
    esac
}

# Get latest release version
get_latest_version() {
    VERSION=$(curl -sSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep '"tag_name"' | cut -d'"' -f4)
}

# Download prebuilt binary
download_binary() {
    BINARY="slashcmd-$PLATFORM"
    URL="https://github.com/$REPO/releases/download/$VERSION/$BINARY"
    CHECKSUM_URL="$URL.sha256"

    echo "Downloading slashcmd $VERSION for $PLATFORM..."

    mkdir -p "$INSTALL_DIR"

    if ! curl -sSL "$URL" -o "$INSTALL_DIR/slashcmd" 2>/dev/null; then
        return 1
    fi

    chmod +x "$INSTALL_DIR/slashcmd"

    # Verify checksum
    echo "Verifying checksum..."
    EXPECTED=$(curl -sSL "$CHECKSUM_URL" 2>/dev/null | cut -d' ' -f1)
    if [ -n "$EXPECTED" ]; then
        if command -v shasum >/dev/null 2>&1; then
            ACTUAL=$(shasum -a 256 "$INSTALL_DIR/slashcmd" | cut -d' ' -f1)
        elif command -v sha256sum >/dev/null 2>&1; then
            ACTUAL=$(sha256sum "$INSTALL_DIR/slashcmd" | cut -d' ' -f1)
        else
            echo "Warning: Could not verify checksum"
            return 0
        fi

        if [ "$EXPECTED" != "$ACTUAL" ]; then
            echo "Checksum mismatch!"
            rm -f "$INSTALL_DIR/slashcmd"
            return 1
        fi
    fi

    return 0
}

# Build from source as fallback
build_from_source() {
    echo "Building from source..."

    if ! command -v cargo >/dev/null 2>&1; then
        echo ""
        echo "Cargo not found. Install Rust first:"
        echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi

    TMP_DIR=$(mktemp -d)
    trap "rm -rf $TMP_DIR" EXIT

    git clone --depth 1 "https://github.com/$REPO.git" "$TMP_DIR" 2>/dev/null || {
        echo "Failed to clone repository"
        exit 1
    }

    cd "$TMP_DIR/cmd"
    cargo build --release --quiet

    mkdir -p "$INSTALL_DIR"
    cp "target/release/slashcmd" "$INSTALL_DIR/"
    chmod +x "$INSTALL_DIR/slashcmd"
}

# Print setup instructions
print_setup() {
    echo ""
    echo "✓ Installed to $INSTALL_DIR/slashcmd"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    # Check if install dir is in PATH
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) IN_PATH=1 ;;
        *) IN_PATH=0 ;;
    esac

    echo "Add to ~/.zshrc or ~/.bashrc:"
    echo ""
    if [ "$IN_PATH" = "0" ]; then
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
    fi
    echo "  /cmd() { slashcmd \"\$@\"; }  # optional shortcut"
    echo ""
    echo "Then: source ~/.zshrc"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "Get started:"
    echo "  slashcmd login"
    echo "  slashcmd find large files"
    echo ""
}

main() {
    echo ""
    echo "  slashcmd installer"
    echo "  https://slashcmd.lgandecki.net"
    echo ""

    detect_platform
    get_latest_version

    if [ -n "$VERSION" ] && download_binary; then
        print_setup
    else
        echo "No prebuilt binary available, building from source..."
        build_from_source
        print_setup
    fi
}

main
