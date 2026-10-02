# Font sources & versions

Committed copies of every font the media pipeline renders with, plus license
files. When bumping a version, re-download from the URL below, refresh the
`.ttf` + license files, and update this table.

## `JetBrainsMono/` - terminal tapes (VHS)

VHS picks the family up via fontconfig (see `tapes/setup-fonts.sh`); the
`Set FontFamily "JetBrains Mono"` in `tapes/_shared.tape` resolves to these
files.

| File | Source | Version |
|------|--------|---------|
| `JetBrainsMono-Regular.ttf` | https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip (`fonts/ttf/`) | 2.304 |
| `JetBrainsMono-Bold.ttf` | same zip | 2.304 |
| `JetBrainsMono-BoldItalic.ttf` | same zip | 2.304 |
| `JetBrainsMono-Italic.ttf` | same zip | 2.304 |
| `OFL.txt` | same zip (root) | license, SIL OFL 1.1 |

## `symbols/` - GUI capture (`BH_GUI_FONT_DIR`)

`gui` probes this directory first when the capture binary sets
`BH_GUI_FONT_DIR`, so status glyphs (⤷ ✂ ⏸ ⟳ ⤬ ⬇) render identically
everywhere.

| File | Source | Version |
|------|--------|---------|
| `NotoSansSymbols2-Regular.ttf` | https://github.com/notofonts/symbols/releases/download/NotoSansSymbols2-v2.008/NotoSansSymbols2-v2.008.zip (`NotoSansSymbols2/hinted/ttf/`) | 2.008 |
| `NotoSansSymbols2-OFL.txt` | same zip (`OFL.txt`) | license, SIL OFL 1.1 |
| `DejaVuSans.ttf` | https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/dejavu-fonts-ttf-2.37.zip (`ttf/`) | 2.37 |
| `DejaVu-LICENSE` | same zip (`LICENSE`) | DejaVu font license (free, permissive) |

## Pinned capture tools

External tools are NOT committed; `bootstrap-tools.sh` (repo root:
`docs/media/`) downloads them into `target/media-tools/bin`. Pins live in that
script; `target/media-tools/VERSIONS.txt` records what was actually installed.

| Tool | Source | Pin |
|------|--------|-----|
| ffmpeg (static amd64) | https://johnvansickle.com/ffmpeg/releases/ffmpeg-7.0.2-amd64-static.tar.xz | 7.0.2 |
| ttyd | https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.x86_64 | 1.7.7 |
| vhs | https://github.com/charmbracelet/vhs/releases/download/v0.12.1/vhs_0.12.1_Linux_x86_64.tar.gz | 0.12.1 |
| chromium (vhs rendering backend) | https://storage.googleapis.com/chromium-browser-snapshots/Linux_x64/1321438/chrome-linux.zip | 1321438 (go-rod v0.116.2 RevisionDefault) |
