#!/usr/bin/env bash
# Build TerminalGoblin and install it for the current user (~/.local).
set -euo pipefail
cd "$(dirname "$0")"

PREFIX="${PREFIX:-$HOME/.local}"

cargo build --release
install -Dm755 target/release/terminalgoblin "$PREFIX/bin/terminalgoblin"
install -Dm644 assets/terminalgoblin.desktop "$PREFIX/share/applications/terminalgoblin.desktop"
install -Dm644 assets/terminalgoblin.svg "$PREFIX/share/icons/hicolor/scalable/apps/terminalgoblin.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$PREFIX/share/applications" || true

echo "Installed to $PREFIX/bin/terminalgoblin"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "Note: $PREFIX/bin is not on your PATH." ;;
esac
