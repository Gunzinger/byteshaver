#!/usr/bin/env bash
# Installs the committed JetBrains Mono TTFs into the user font directory so
# fontconfig-based renderers (VHS/ttyd/chrome via `Set FontFamily "JetBrains Mono"`)
# resolve the pinned family without any system font packages.
#
# Usage: docs/media/tapes/setup-fonts.sh
# Override the target dir with FONT_HOME (default: $HOME/.fonts, which
# fontconfig scans automatically; fc-cache only forces a rebuild of the cache).
#
# The xtask media pipeline runs this once per staged environment with the
# isolated HOME, before invoking vhs.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Default: the committed font dir next to this script (repo layout). Override
# with BH_TAPE_FONT_DIR when the fonts live elsewhere (e.g. a staged copy).
FONT_SRC="${BH_TAPE_FONT_DIR:-$SCRIPT_DIR/../fonts/JetBrainsMono}"
FONT_HOME="${FONT_HOME:-$HOME/.fonts}"

mkdir -p "$FONT_HOME"
cp -f "$FONT_SRC"/*.ttf "$FONT_HOME/"

if command -v fc-cache >/dev/null 2>&1; then
    fc-cache -f "$FONT_HOME" >/dev/null
fi
echo "fonts installed into $FONT_HOME: $(ls "$FONT_HOME"/*.ttf | wc -l) file(s)"
