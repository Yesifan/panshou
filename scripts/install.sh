#!/bin/sh

set -eu

REPOSITORY="${PANSOU_REPOSITORY:-Yesifan/panshou}"
VERSION="${PANSOU_VERSION:-latest}"
INSTALL_DIR=
INSTALL_DIR_EXPLICIT=0
if [ -n "${PANSOU_INSTALL_DIR:-}" ]; then
    INSTALL_DIR=$PANSOU_INSTALL_DIR
    INSTALL_DIR_EXPLICIT=1
fi

usage() {
    cat <<'EOF'
Install PanSou from GitHub Releases.

Usage: install.sh [--version VERSION] [--dir DIRECTORY]

Options:
  --version VERSION  Release to install, for example v0.1.0 (default: latest)
  --dir DIRECTORY    Installation directory; skips automatic PATH detection
                     (default: $HOME/.local/bin, then /usr/local/bin, if in PATH)
  -h, --help         Show this help

Environment variables:
  PANSOU_VERSION      Same as --version
  PANSOU_INSTALL_DIR  Same as --dir
EOF
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || fail "--version requires a value"
            VERSION=$2
            shift 2
            ;;
        --dir)
            [ "$#" -ge 2 ] || fail "--dir requires a value"
            [ -n "$2" ] || fail "--dir requires a non-empty value"
            INSTALL_DIR=$2
            INSTALL_DIR_EXPLICIT=1
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

case "$(uname -s)" in
    Linux) OS=linux ;;
    Darwin) OS=macos ;;
    *) fail "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
    x86_64|amd64) ARCH=x86_64 ;;
    arm64|aarch64) ARCH=aarch64 ;;
    *) fail "unsupported CPU architecture: $(uname -m)" ;;
esac

if command -v curl >/dev/null 2>&1; then
    download() {
        curl --fail --location --silent --show-error "$1" --output "$2"
    }
elif command -v wget >/dev/null 2>&1; then
    download() {
        wget --quiet --output-document="$2" "$1"
    }
else
    fail "curl or wget is required"
fi

if command -v shasum >/dev/null 2>&1; then
    checksum() {
        ACTUAL=$(shasum -a 256 "$2" | awk '{ print $1 }')
        [ "$ACTUAL" = "$1" ]
    }
elif command -v sha256sum >/dev/null 2>&1; then
    checksum() {
        ACTUAL=$(sha256sum "$2" | awk '{ print $1 }')
        [ "$ACTUAL" = "$1" ]
    }
else
    fail "sha256sum or shasum is required"
fi

path_contains() {
    case ":${PATH:-}:" in
        *:"$1":*) return 0 ;;
        *) return 1 ;;
    esac
}

if [ "$INSTALL_DIR_EXPLICIT" -eq 0 ]; then
    if [ -n "${HOME:-}" ] && path_contains "$HOME/.local/bin"; then
        INSTALL_DIR="$HOME/.local/bin"
    elif path_contains /usr/local/bin; then
        INSTALL_DIR=/usr/local/bin
    else
        fail "neither \$HOME/.local/bin nor /usr/local/bin is in PATH
add one of them to PATH, or specify a directory with --dir or PANSOU_INSTALL_DIR"
    fi
fi

case "$VERSION" in
    latest) RELEASE_PATH=latest/download ;;
    v*) RELEASE_PATH="download/$VERSION" ;;
    *) RELEASE_PATH="download/v$VERSION" ;;
esac

ASSET="pansou-$OS-$ARCH.tar.gz"
BASE_URL="https://github.com/$REPOSITORY/releases/$RELEASE_PATH"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/pansou-install.XXXXXX")
trap 'rm -rf "$TMP_DIR"' 0 HUP INT TERM

printf 'Downloading %s...\n' "$ASSET"
download "$BASE_URL/$ASSET" "$TMP_DIR/$ASSET"
download "$BASE_URL/SHA256SUMS" "$TMP_DIR/SHA256SUMS"

EXPECTED=$(awk -v asset="$ASSET" '$2 == asset || $2 == "*" asset { print $1; exit }' "$TMP_DIR/SHA256SUMS")
[ -n "$EXPECTED" ] || fail "$ASSET is missing from SHA256SUMS"
checksum "$EXPECTED" "$TMP_DIR/$ASSET" || fail "checksum verification failed for $ASSET"

tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
[ -f "$TMP_DIR/pansou" ] || fail "release archive does not contain pansou"
chmod +x "$TMP_DIR/pansou"

mkdir -p "$INSTALL_DIR"
DESTINATION="$INSTALL_DIR/pansou"
if command -v install >/dev/null 2>&1; then
    install -m 0755 "$TMP_DIR/pansou" "$DESTINATION"
else
    cp "$TMP_DIR/pansou" "$DESTINATION"
    chmod 0755 "$DESTINATION"
fi

printf 'Installed PanSou to %s\n' "$DESTINATION"
