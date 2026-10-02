#!/usr/bin/env bash
# Bootstraps the pinned external tools needed by the media capture pipeline
# (vhs terminal tapes, xtask `media` post-processing) into
# `target/media-tools/bin`. No root required; everything is user-local.
#
# Usage:
#   docs/media/bootstrap-tools.sh            # install missing tools (idempotent)
#   docs/media/bootstrap-tools.sh --force    # re-download even if present
#
# After running, add the bin dir to PATH for tape rendering:
#   export PATH="$PWD/target/media-tools/bin:$PATH"
#
# Pin bumping: edit the *_VERSION variables below. Each has a comment with
# where to find release assets. Record the bump in docs/media/fonts/SOURCES.md
# (tools section) so `xtask media deps` stays traceable to a commit.
#
# Tools installed:
#   ffmpeg 7.0.2 (static, johnvansickle.com)  - frame post-processing + vhs dep
#   ttyd   1.7.7 (tsl0922/ttyd releases)      - terminal backend for vhs
#   vhs    0.12.1 (charmbracelet/vhs releases)- tape renderer
#   chrome 1321438 (chromium-browser-snapshots) - vhs renders via headless
#     Chromium (go-rod); this is the exact revision go-rod v0.116.2 (vhs
#     0.12.1's dependency) would auto-download, pinned here so runs stay
#     hermetic and offline after bootstrap. Exposed as `bin/chrome` (symlink)
#     because go-rod's LookPath() resolves the bare name `chrome` on PATH.
#
# NOTE: VHS only passes --no-sandbox to the browser when `VHS_NO_SANDBOX` is
# set (vhs.go: NoSandbox(os.Getenv("VHS_NO_SANDBOX") != "")). Containers and
# CI runners without user namespaces NEED it:
#   export VHS_NO_SANDBOX=1
set -euo pipefail

FFMPEG_VERSION="7.0.2"     # https://johnvansickle.com/ffmpeg/releases/ (static builds; a pinned "ffmpeg-<ver>-amd64-static.tar.xz" exists per release)
TTYD_VERSION="1.7.7"       # https://github.com/tsl0922/ttyd/releases (asset: ttyd.x86_64)
VHS_VERSION="0.12.1"       # https://github.com/charmbracelet/vhs/releases (asset: vhs_<ver>_Linux_x86_64.tar.gz, bare `vhs` inside)
CHROMIUM_REVISION="1321438" # https://storage.googleapis.com/chromium-browser-snapshots/Linux_x64/<rev>/chrome-linux.zip (go-rod v0.116.2 RevisionDefault)

FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

# Repo root = two levels above this script (docs/media/bootstrap-tools.sh).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TOOLS_DIR="${BH_MEDIA_TOOLS_DIR:-$ROOT_DIR/target/media-tools}"
BIN_DIR="$TOOLS_DIR/bin"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

mkdir -p "$BIN_DIR"
VERSIONS_FILE="$TOOLS_DIR/VERSIONS.txt"
: > "$VERSIONS_FILE"

fetch() { # fetch <url> <dest>
    local url="$1" dest="$2"
    if [ ! -s "$dest" ] || [ "$FORCE" -eq 1 ]; then
        echo "downloading $url"
        curl -fsSL --retry 3 -o "$dest" "$url"
    fi
}

installed() { # installed <name> -> 0 if present and FORCE not set
    [ "$FORCE" -ne 1 ] && [ -x "$BIN_DIR/$1" ]
}

# --- ffmpeg (static) -------------------------------------------------------
# The release tarball unpacks to ffmpeg-<ver>-amd64-static/{ffmpeg,ffprobe}.
if ! installed ffmpeg || ! installed ffprobe; then
    fetch "https://johnvansickle.com/ffmpeg/releases/ffmpeg-${FFMPEG_VERSION}-amd64-static.tar.xz" \
        "$TMP_DIR/ffmpeg.tar.xz"
    tar -xJf "$TMP_DIR/ffmpeg.tar.xz" -C "$TMP_DIR" \
        "ffmpeg-${FFMPEG_VERSION}-amd64-static/ffmpeg" \
        "ffmpeg-${FFMPEG_VERSION}-amd64-static/ffprobe"
    mv "$TMP_DIR/ffmpeg-${FFMPEG_VERSION}-amd64-static/ffmpeg" "$BIN_DIR/ffmpeg"
    mv "$TMP_DIR/ffmpeg-${FFMPEG_VERSION}-amd64-static/ffprobe" "$BIN_DIR/ffprobe"
    chmod +x "$BIN_DIR/ffmpeg" "$BIN_DIR/ffprobe"
fi

# --- ttyd (single static binary) --------------------------------------------
if ! installed ttyd; then
    fetch "https://github.com/tsl0922/ttyd/releases/download/${TTYD_VERSION}/ttyd.x86_64" \
        "$BIN_DIR/ttyd"
    chmod +x "$BIN_DIR/ttyd"
fi

# --- vhs (Go binary inside a tar.gz) ----------------------------------------
if ! installed vhs; then
    fetch "https://github.com/charmbracelet/vhs/releases/download/v${VHS_VERSION}/vhs_${VHS_VERSION}_Linux_x86_64.tar.gz" \
        "$TMP_DIR/vhs.tar.gz"
    tar -xzf "$TMP_DIR/vhs.tar.gz" -C "$TMP_DIR" "vhs_${VHS_VERSION}_Linux_x86_64/vhs"
    mv "$TMP_DIR/vhs_${VHS_VERSION}_Linux_x86_64/vhs" "$BIN_DIR/vhs"
    chmod +x "$BIN_DIR/vhs"
fi

# --- chromium (rendering backend vhs drives through go-rod) ------------------
# The zip unpacks to chrome-linux/; it must stay a directory (chrome needs its
# resources alongside the binary), so it lives outside bin/ and is exposed via
# a `chrome` symlink that go-rod's exec.LookPath("chrome") finds on PATH.
if ! installed chrome; then
    fetch "https://storage.googleapis.com/chromium-browser-snapshots/Linux_x64/${CHROMIUM_REVISION}/chrome-linux.zip" \
        "$TMP_DIR/chrome-linux.zip"
    rm -rf "$TOOLS_DIR/chrome-linux"
    unzip -q "$TMP_DIR/chrome-linux.zip" -d "$TOOLS_DIR"
    ln -sfn "../chrome-linux/chrome" "$BIN_DIR/chrome"
fi

# --- verify + record ---------------------------------------------------------
# Versions are parsed from the tools themselves (not trusted from this script)
# so VERSIONS.txt always reflects what is actually on disk.
fail() { echo "ERROR: $1" >&2; exit 1; }

FFMPEG_ACTUAL="$("$BIN_DIR/ffmpeg" -version 2>/dev/null | head -1)" \
    || fail "ffmpeg -version failed"
FFPROBE_ACTUAL="$("$BIN_DIR/ffprobe" -version 2>/dev/null | head -1)" \
    || fail "ffprobe -version failed"
TTYD_ACTUAL="$("$BIN_DIR/ttyd" --version 2>/dev/null | head -1)" \
    || fail "ttyd --version failed"
VHS_ACTUAL="$("$BIN_DIR/vhs" --version 2>/dev/null | head -1)" \
    || fail "vhs --version failed"
CHROME_ACTUAL="$("$BIN_DIR/chrome" --version 2>/dev/null | head -1)" \
    || fail "chrome --version failed (did the chromium snapshot unpack?)"

{
    echo "# Pinned media tool versions (written by docs/media/bootstrap-tools.sh)"
    echo "# Pins: ffmpeg=$FFMPEG_VERSION ttyd=$TTYD_VERSION vhs=$VHS_VERSION chromium=$CHROMIUM_REVISION"
    echo "$FFMPEG_ACTUAL"
    echo "$FFPROBE_ACTUAL"
    echo "$TTYD_ACTUAL"
    echo "$VHS_ACTUAL"
    echo "$CHROME_ACTUAL"
} > "$VERSIONS_FILE"

echo "media tools ready in $BIN_DIR:"
sed 's/^/  /' "$VERSIONS_FILE"
echo
echo "remember: export VHS_NO_SANDBOX=1 and PATH=\"$BIN_DIR:\$PATH\" before running vhs"
