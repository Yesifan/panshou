#!/bin/sh

set -eu

INSTALL_DIR="${PANSOU_INSTALL_DIR:-}"

usage() {
    cat <<'EOF'
Uninstall PanSou and remove its user data.

Usage: uninstall.sh [--dir DIRECTORY]

Options:
  --dir DIRECTORY  Directory containing the pansou binary
  -h, --help       Show this help

Environment variables:
  PANSOU_INSTALL_DIR  Same as --dir
EOF
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --dir)
            [ "$#" -ge 2 ] || fail "--dir requires a value"
            [ -n "$2" ] || fail "--dir requires a non-empty value"
            INSTALL_DIR=$2
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

: "${HOME:?HOME is required to locate PanSou data}"

if [ -z "$INSTALL_DIR" ]; then
    if [ -e "$HOME/.local/bin/pansou" ]; then
        INSTALL_DIR="$HOME/.local/bin"
    elif [ -e /usr/local/bin/pansou ]; then
        INSTALL_DIR=/usr/local/bin
    fi
fi

case "$(uname -s)" in
    Linux)
        CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/pansou"
        STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/pansou"
        CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/pansou"
        ;;
    Darwin)
        CONFIG_DIR="$HOME/Library/Application Support/pansou"
        STATE_DIR=$CONFIG_DIR
        CACHE_DIR="$HOME/Library/Caches/pansou"
        ;;
    *)
        fail "unsupported operating system: $(uname -s)"
        ;;
esac

remove_file() {
    [ -e "$1" ] || return 0
    rm "$1"
    printf 'Removed %s\n' "$1"
}

remove_data_dir() {
    [ -e "$1" ] || return 0
    case "$1" in
        */pansou) ;;
        *) fail "refusing to remove unexpected data path: $1" ;;
    esac
    rm -r "$1"
    printf 'Removed %s\n' "$1"
}

if [ -n "$INSTALL_DIR" ]; then
    remove_file "$INSTALL_DIR/pansou"
fi

remove_data_dir "$CONFIG_DIR"
if [ "$STATE_DIR" != "$CONFIG_DIR" ]; then
    remove_data_dir "$STATE_DIR"
fi
remove_data_dir "$CACHE_DIR"

printf 'PanSou uninstalled.\n'
