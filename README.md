# byteshaver 🗜️

`byteshaver` is a command-line utility focusing on converting images into other formats,
 specifically focusing on support for modern image standards and encoders.

`byteshaver` simplifies the process of batch converting images,
 optimizing for both performance and storage efficiency.

> [!NOTE]
> **Heritage:** this project started out as a fork of [imgc-rs](https://github.com/tduyng/imgc-rs) by [tduyng](https://github.com/tduyng).
> Over time it has diverged too much from the original, so it is no longer a fork
> and is now developed independently as **byteshaver**.
> Thanks to [@tduyng](https://github.com/tduyng) for the original work and the head start!

### Usage example using the docker container:

```bash
> docker run -v ./examples/:/targets/ -it gunzinger/byteshaver:latest byteshaver "**/*.*" avif
Converting 16 files...
Using "ravif" (0.13.0) with options (quality: 90, speed: 3, bit depth: Eight, color model: RGB)
Encode statistics:
Successful: 15
Skipped:    0
Errors:     0
Total input size:  24.0 MiB
Total output size: 13.2 MiB
Compression ratio: 54.95%
```

---

## Key Features 🧰

- **Broad Format Support**: 
 Works with many [supported image formats](#supported-formats) — including JPEG XL and
 HEIC/HEIF/AVIF input, and modern targets like AVIF, JPEG XL and optimized PNG (oxipng).
- **EXIF control**: strip (default), keep, or filter specific tags — with orientation
  baked into the pixels when the tag is dropped.
- **Animation support**: re-encode animated GIF/WebP/APNG input to animated WebP, APNG or GIF
  with frame timing and loop count preserved.
- **Desktop GUI**: drag-and-drop files or folders onto a queue-centric window
  ([gui/README.md](gui/README.md)); conversions are byte-identical to the CLI.
- **Works with huge images**:
  Can optimize very large images (~1GiB input image size, ~32Kx~16K px dimensions).
- **Speedy Processing**:
  Written in Rust to keep overhead to a minimum, we also take advantage of `rayon` for parallel processing.
- **Input selection using Glob Patterns**:
  Target selection is made intuitive for cli enthusiasts via glob patterns.
- **Scriptable**: machine-readable JSON-lines event logs via `--json-log`, typed job API
  (`byteshaver::job`) for embedding the converter in other Rust software.
- **Custom Output**:
 Choose where your converted images are saved.

---

## Supported formats

### Input formats 🖼️

To keep it simple: `JPEG`, `PNG` (incl. APNG), `GIF`, `WebP` (incl. animated), `JPEG XL`, `BMP`, `DDS`, `Farbfeld`, `HDR`, `ICO`, `EXR`, `PNM`, `QOI`, `TGA`, `TIFF`

Input images are decoded using the `image` crate,
 please see [their documentation for supported image formats](https://docs.rs/image/0.25.6/image/codecs/index.html#supported-formats).

Additionally, `HEIC`, `HEIF`, `HIF` and `AVIF` (still images) are decoded through `libheif`
when the `dec-heif` feature is compiled in:

- multi-image files (e.g. iPhone bursts) can be expanded into one output per image
  via `--heif-image-policy all` (outputs are then named `stem.ext`, `stem_1.ext`, ...);
  by default only the primary image is converted
- EXIF metadata is extracted (with the rotation already baked in by the decoder), XMP
  and ICC profiles are carried over where the target format supports them
- 10/12-bit (HDR) sources are down-converted to 8-bit with a printed warning

Without the feature, HEIC/HEIF/AVIF inputs fail per-file with a clear message while the
rest of the batch keeps converting. Please note that the **static release binaries are
built without** `dec-heif` (the native libheif + codec libraries have no static archives),
while the **docker images include it** and are validated end-to-end by CI.

`JPEG XL` (`.jxl`) inputs are decoded with [`jxl-oxide`](https://crates.io/crates/jxl-oxide)
 (still images and animations, 8/16-bit, EXIF/XMP boxes and ICC profiles).
 Requires the `jxl` feature (enabled by default).

### Output formats 📤

- `webp`, webp encoder using the `webp` crate (libwebp bindings) - offers lossy and lossless encoding
- `webp-image`, webp encoder using the `image` crate - offers lossless encoding
- `avif`, avif encoder using the `ravif` crate - offers lossy and lossless encoding. EXIF that
  survives the policy is embedded natively by ravif (>= 0.13) as a standard HEIF `Exif` item after
  the AV1 encode — with no measurable encoding overhead.
- `png`, png encoder using the `png` crate - offers lossless encoding (with optional eXIf embedding)
- `jpeg`, jpeg optimizer using the `mozjpeg` crate - only optimizes images (with optional EXIF embedding)
- `jxl`, jpeg-xl encoder using in-tree FFI bindings to `libjxl` (vendored static build via `jpegxl-src`) - offers
  lossy and lossless encoding, real animated output (frame durations), EXIF embedding and the full
  `JxlEncoderFrameSettingId` surface via repeatable `--setting ID=VALUE` flags. Requires the `jxl` feature
  (enabled by default) and `cmake`, a C++ compiler and `nasm` at build time.
- `oxipng`, optimal lossless PNG compression using the `oxipng` crate - PNG→PNG inputs are re-optimized
  byte-level passthrough (bit-exact, preserves palette/tRNS/APNG chunks); other inputs are transcoded
  first. Requires the `opt-oxipng` feature (enabled by default).
- `webp-anim`, animated webp encoder using the `webp-animation` crate - animated input
  (gif/animated webp/APNG) with frame timing + loop count preserved. Requires `anim-webp` (default).
- `apng`, animated PNG encoder using the `png` crate - animated input with millisecond-exact
  delays and loop count preserved. Requires `anim-apng` (default).
- `gif`, (animated) gif encoder using the `image` crate - delays rounded to the GIF-standard 10 ms.

#### Output format notes 📝

When working with very large input images, please keep in mind the output format limits.
In particular:
- `webp`: maximum dimension of [16384x16384px](https://www.ietf.org/rfc/rfc9649.pdf#name-riff-header)
- `avif`: maximum dimension of [65536x65536px](https://aomediacodec.github.io/av1-avif/#profiles-overview), note 
  [Baseline Profile](https://aomediacodec.github.io/av1-avif/#baseline-profile)
  and [Advanced Profile](https://aomediacodec.github.io/av1-avif/#advanced-profile)
  limits if you want to be friendly to consuming hardware decoders. :)
- `jxl`: maximum dimension of [262144x262144px](https://docs.rs/libjxl/latest/jxl/schema.html) (level 5 codestream);
  animation frame delays are stored in millisecond ticks (sub-millisecond remainders are truncated)

### JPEG XL notes 📝

The `jxl` target mirrors the `cjxl` option surface:

- `--quality` (JPEG-style 0-100) and `--distance` (Butteraugli 0.0-25.0) are mutually exclusive;
  the mapping between them follows libjxl's own `JxlEncoderDistanceFromQuality`.
- `--lossless` enables true bit-exact encoding (implies `--original-profile`).
- `--effort 1..=10` (default 7), `--decoding-speed 0..=4` (default 0).
- `--color-encoding` selects sRGB/linear sRGB (grayscale flavors for gray inputs) or
  `--color-encoding icc-passthrough` to embed the source ICC profile verbatim.
- EXIF metadata follows the global `--exif` policy and is embedded as a (Brotli-compressed)
  `Exif` metadata box, which automatically enables the box-based container.
- Advanced libjxl frame settings can be passed through verbatim, e.g. `--setting brotli_effort=9`
  or `--setting modular=1` (ids resolve case-insensitively; unknown ids print the full list).
- Building requires `cmake`, a C++ compiler and `nasm` (the vendored libjxl is compiled once
  during the cargo build).

## Animations 🎞️

Animated inputs (`gif`, animated `webp`, `APNG`) are decoded with frame timing and
loop count preserved, and can be re-encoded to animated targets:

| target command | animated output | container notes |
|----------------|-----------------|-----------------|
| `webp-anim`    | yes (webp-animation/libwebp) | loop count preserved; EXIF embedded via a `EXIF` RIFF chunk |
| `apng`         | yes (png crate)          | `.png` output; a separate default image keeps non-APNG viewers happy; EXIF embedded as `eXIf` chunk |
| `gif`          | yes (image crate)        | no EXIF support |

Rounding notes:
- internal timing is millisecond-exact; `gif` targets round down to the
  GIF-standard 10 ms granularity (minimum 10 ms per frame),
- `apng` writes millisecond-exact delays (1/1000 s fractions),
- animated `webp` keeps millisecond-exact cumulative timestamps.

Converting animated input to a target that cannot encode animations
(`jpeg`, `avif`, still `webp`/`png`, ...) defaults to encoding the first frame
with a notice. Use `--animated-input error` to fail such files instead
(e.g. in pipelines that must never silently drop animation), and
`--max-animation-memory <MiB>` (default 4096) to bound the memory used by
decoded frame buffers.

## EXIF metadata 📸

EXIF handling is controlled by global flags that work with every conversion
command:

```bash
# strip all EXIF (this is the default) - orientation is baked into the pixels
byteshaver "photos/**/*.jpg" webp

# keep EXIF verbatim and embed it into the output
byteshaver "photos/**/*.jpg" webp --exif keep

# keep everything except location and camera orientation
byteshaver "photos/**/*.jpg" webp --exif filter --exif-except gps,GPSInfo,Orientation

# keep only a whitelist of tags
byteshaver "photos/**/*.jpg" webp --exif filter --exif-only DateTimeOriginal,Make,Model

# print all recognized tag names (also IFD wildcards: ifd0, exif, gps, ...)
byteshaver --exif-list-tags
```

Policies:

- `strip` (default): removes all EXIF from outputs. Privacy-safe: before the
  pixels are re-encoded, the `Orientation` tag is applied to the image data, so
  photos never appear rotated/sideways in software that ignores EXIF.
- `keep`: copies EXIF verbatim into outputs where the container supports it;
  pixels are left untouched (viewers rotate using the Orientation tag).
- `filter`: keep all tags except `--exif-except <TAGS>`, or keep only
  `--exif-only <TAGS>` (mutually exclusive). Tag lists accept comma-separated
  tag names (see `--exif-list-tags`), IFD wildcards (`ifd0`, `exif`, `gps`,
  `interop`) and numeric tags (`0x8825`). Unknown names abort with the
  recognized set.

Where EXIF ends up per target:

| target | EXIF embedding |
|--------|----------------|
| `jpeg` | APP1 segment |
| `png`, `apng` | `eXIf` chunk |
| `oxipng` | `eXIf` chunk; under `keep`/`filter` metadata stripping is forced off, under `strip` an unset `--strip` is bumped to `safe` |
| `webp`, `webp-image`, `webp-anim` | `EXIF` RIFF chunk |
| `jxl` | `Exif` metadata box (Brotli-compressed) |
| `avif` | standard HEIF `Exif` item (ISOBMFF), embedded natively by `ravif` after the AV1 encode |
| `gif` | not supported — a warning is printed per file and the count appears in the run summary |

Notes:
- inputs without EXIF simply produce outputs without EXIF, regardless of policy
  (no synthetic metadata is created),
- when a `filter` result cannot be re-serialized, the verbatim EXIF is kept and
  a warning is printed (EXIF is never silently lost),
- ICC profiles are carried over where supported (e.g. `jxl`); a dedicated ICC
  policy flag may arrive later.

## Overwrite & collision behavior ♻️

By default an existing output file is never touched (the file is reported as
skipped). This can be tuned:

- `--overwrite-if-smaller`: replace an existing output when the new encode is
  smaller (keeps the best result of multiple runs),
- `--overwrite-existing`: always replace existing outputs,
- `--discard-if-larger-than-input`: don't write the output at all when the
  encode is bigger than the input file (useful when re-optimizing
  already-compressed directories).

Collisions (e.g. `a.jpg` and `a.png` in one batch both map to `a.webp`): the
first conversion wins, later ones are reported as skipped with a notice.
Exception: multi-image HEIF expansion (`--heif-image-policy all`) always uses
distinct names (`stem.ext`, `stem_1.ext`, ...).

## JSON logs

Every conversion command accepts the global `--json-log <PATH>` flag (enabled
in default builds): alongside the regular terminal output, one JSON object per
progress event is appended per line (JSON lines), e.g.

```
byteshaver "images/**/*.jpg" webp -o out --json-log run.jsonl
```

Each line is flushed immediately, so the log can be tailed while a batch
runs; events include `Started`, `FileStarted`, `FileFinished` (with the
per-file outcome), `ProgressStats` (running byte/count totals), `Notice`
(warnings) and `Finished`.

## GUI (desktop) 🖱️

`byteshaver` ships a desktop GUI as a separate workspace crate
(`gui/` = `byteshaver-gui`, egui/eframe): a queue-centric single window where
the **entire window is a drag-and-drop target** — drop files *or folders*
anywhere (folders are expanded recursively by the core), or use the native
file picker. One global target format with its full option surface, EXIF /
collision / animation policies, live per-file progress, cancel, and a report
panel with JSONL export. Conversions run through the same headless core the
CLI uses, so outputs are byte-identical.

<!-- TODO: replace with a real screenshot
![byteshaver-gui screenshot](docs/gui-screenshot.png)
-->

Build it from source:

```bash
cargo build --release -p byteshaver-gui
# binary: target/release/byteshaver-gui
```

Notes:

- Linux: needs GL at runtime (X11 or Wayland); file dialogs use the
  xdg-desktop-portal service. No GTK development packages are required to
  compile.
- The encoder list reflects the build: encoders compiled out (e.g. a build
  without `jxl`) are shown grayed-out with the reason, and HEIC/HEIF/AVIF
  input is only offered when the core was built with the `dec-heif` feature.
- Settings (output dir, encoder + options, policies, window size) persist in
  the OS config dir under `byteshaver-gui/settings.json`.
- GUI binaries are published for every release alongside the CLI (see
  [Installation](#using-published-binaries--)); Linux builds use the X11
  windowing backend. See [gui/README.md](gui/README.md) for build details.

## Requests

If this does not cover your needs,
 please feel free to open an issue to request additional input and/or output formats.

I am focusing on supporting modern image formats supported in browsers,
 as this tool is optimally suited for optimizing static directories for different web apps.

For a good overview of browser support, see the [caniuse.com](https://caniuse.com) pages
 for different images, e.g.: [avif](https://caniuse.com/avif), [webp](https://caniuse.com/webp).

---

## Installation 💾

### Using published binaries 📡

Binaries for Windows and Linux are built for every tag — the classic
CLI binary **and** the desktop GUI as separate downloads:

- `byteshaver-<version>[-cpu]` — CLI, Linux (musl, static-pie)
- `byteshaver-<version>[-cpu].exe` — CLI, Windows
- `byteshaver-gui-<version>[-cpu]` — desktop GUI, Linux (musl, static-pie, X11)
- `byteshaver-gui-<version>[-cpu].exe` — desktop GUI, Windows
- `...-packed[.exe]` variants of each of the above — optional self-extracting
  packed copies (65–70 % smaller download/disk at the cost of ~9 ms slower
  process startup on Linux)

The default artifacts are **not** packed: measurements
([docs/zstd-packer-analysis.md](docs/zstd-packer-analysis.md),
[docs/upx-binary-size-report.md](docs/upx-binary-size-report.md)) show packed
variants pay a fixed per-invocation decompression toll. The `-packed`
variants are produced by the in-repo zstd packer
([tools/packer](tools/packer)) in a dedicated CI job for size-constrained
setups; they are plain zstd frames and can be restored with `zstd -d`.

`-cpu` suffixes (x86-64-v4, znver3, znver5) are tuned builds; the suffixless
artifacts target x86-64-v3 (Intel Haswell / AMD Zen and newer). Every artifact
ships with a `.sha256` checksum. Third-party code embedded by the packer
(zstd, BSD-2-Clause) is attributed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Feature notes: the static Linux/Windows binaries include everything except
`dec-heif` — HEIC/HEIF/AVIF input requires the native libheif libraries and is
therefore only shipped in the **docker images** (validated by CI end-to-end,
see below). The Linux GUI build uses the X11 windowing backend.

See the [GitHub releases](https://github.com/Gunzinger/byteshaver/releases) page for downloads.

### Using the docker image 🐳

Docker containers are also built for every tag. Both the alpine and debian
images include HEIC/HEIF/AVIF input support (`dec-heif`): CI builds both
images and validates the complete chain — including a real HEIC file decoded
inside the container — before they are published.

See the [Docker Hub](https://hub.docker.com/r/gunzinger/byteshaver) page for available tags.

```bash
docker run -it gunzinger/byteshaver:latest byteshaver --help

# directory passthrough on linux
docker run -v ./input-folder/:/targets/ -it gunzinger/byteshaver:latest byteshaver "/targets/**/*.png" avif

# note that on windows the volume passthroughs need to have absolute paths, e.g. (for powershell)
docker run -v ${PWD}/input-folder/:/targets/ -it gunzinger/byteshaver:latest byteshaver "/targets/**/*.png" avif

```

---

## How to Use `byteshaver` 🧑‍💻

### Basic Usage

The `byteshaver` program uses glob patterns for target selection:

```bash
byteshaver "examples/**/*.png" webp
byteshaver "examples/**/*.jpg" webp
byteshaver "examples/**/*" webp
```

### Specifying an output directory 🗃️

```bash
byteshaver "examples/**/*" webp -o output_images
```

### Common recipes 🍳

```bash
# re-optimize existing PNGs in place (bit-exact, never grows)
byteshaver "static/**/*.png" oxipng --level 4 --overwrite-if-smaller

# shrink a photo library to jpeg-xl at explicit quality
byteshaver "photos/**/*.jpg" jxl --quality 85

# convert a folder of gifs to animated webp (timing + loop count preserved)
byteshaver "gifs/**/*.gif" webp-anim --quality 80

# strip location data from everything that gets re-encoded,
# keep all other EXIF where the target supports it
byteshaver "camera/**/*" avif --exif filter --exif-except gps,GPSInfo

# pipeline-friendly run: fail on dropped animations, log every event
byteshaver "site/**/*" avif --animated-input error --json-log run.jsonl
```

### Cleaning up generated files 🧹

**Warning**: Use this command with caution. This is basically `rm -rf` with regex.

```bash
byteshaver "examples/**/*.webp" clean
```

---

### Command Help 📖

For detailed command usage, see all arguments with `--help` or `-h`:

```bash
❯ byteshaver --help
  A configurable and efficient batch image converter written in Rust.
  
  Usage: byteshaver [OPTIONS] <PATTERN> <COMMAND>
  
  Commands:
    webp        Convert images to webp format (using webp crate)
    webp-image  Convert images to webp format (using image crate)
    avif        Convert images to avif format (using ravif crate)
    png         Convert images to png format (using image crate)
    jpeg        Convert images to optimized jpeg format (using mozjpeg crate)
    jxl         Convert images to jpeg-xl format (using libjxl)
    oxipng      Convert images to optimally compressed png format (using oxipng)
    webp-anim   Convert images to animated webp format (using webp-animation crate). Supports animated input (gif, animated webp, APNG); the source loop count is preserved. Still input becomes a 1-frame animation
    apng        Convert images to animated png format (APNG, using png crate). Supports animated input (gif, animated webp, APNG); the source loop count is preserved. Still input becomes a 1-frame animation. The default image stays readable by non-APNG-aware viewers
    gif         Convert images to (animated) gif format (using image crate). Supports animated input; the source loop count is preserved. Note: frame delays are rounded to the GIF-standard 10 ms granularity
    clean       Remove files matching a glob pattern
    help        Print this message or the help of the given subcommand(s)
  
  Arguments:
    <PATTERN>
            Glob pattern to match images to convert. Example: `images/**/*.png`
  
  Options:
    -o, --output <OUTPUT>
            Output directory (flat) of processed images. Defaults to the same location as the original images with the new file extension. If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
  
        --reverse-processing-order
            By default, byteshaver will process input files in lexicographical order after expanding the pattern. Setting this starts the process from the back
  
        --overwrite-if-smaller
            Overwrite the existing output file if the current conversion resulted in a smaller file
  
        --overwrite-existing
            Overwrite existing output files regardless of size
  
        --discard-if-larger-than-input
            Discards the encoding result if it is larger than the input file (does not create an output file)
  
        --discard-input-alpha-channel
            Discards the alpha channel of the input image(s) if it is present. (this does not make loading faster, but it can improve the encoding result)
  
        --exif <keep|strip|filter>
            EXIF metadata handling policy (global). strip: remove all EXIF and bake the orientation into the pixels (default, privacy-safe). keep: copy EXIF verbatim into outputs where the container supports it. filter: keep all tags except --exif-except <TAGS>, or only --exif-only <TAGS>
  
            Possible values:
            - keep:   Copy EXIF verbatim into outputs that support it
            - strip:  Remove all EXIF from outputs (default)
            - filter: Keep all but `--exif-except <TAGS>`, or only `--exif-only <TAGS>`
  
        --exif-except <TAGS>
            Comma-separated EXIF tags/IFDs to drop when using --exif filter (e.g. --exif-except gps,GPSInfo,Orientation; names from --exif-list-tags, IFD wildcards ifd0/ifd1/exif/gps/interop, or numeric tags like 0x8825). Mutually exclusive with --exif-only; requires --exif filter
  
        --exif-only <TAGS>
            Comma-separated EXIF tags/IFDs to keep when using --exif filter (same syntax as --exif-except). Mutually exclusive with --exif-except; requires --exif filter
  
        --exif-list-tags
            Print recognized EXIF tag names and exit
  
        --animated-input <first-frame|error>
            How to treat animated input (gif / animated webp / APNG) when the selected target cannot encode animations. first-frame: encode the first frame only, with a notice (default). error: fail the file with an error instead of silently dropping the animation (for pipelines that must never drop frames)
  
            Possible values:
            - first-frame: Encode the first frame only, printing a notice (default)
            - error:       Fail the file with an error instead of silently dropping animation
  
        --max-animation-memory <MIB>
            Hard cap for decoded animation memory in MiB (frames are RGBA: width x height x 4 x frames). Files whose projected frame buffers exceed the cap fail with an error instead of risking an OOM. Defaults to 4096
  
        --json-log <PATH>
            Write one JSON object per progress event to this file (JSON lines) in addition to the regular stdout output (requires the `logs` feature, on by default)
  
    -h, --help
            Print help (see a summary with '-h')
  
    -V, --version
            Print version

> Note: HEIC/HEIF-enabled builds (`dec-heif` feature, docker images) additionally
> expose the global `--heif-image-policy <primary|all>` flag.
```

Command-specific options (every command also accepts all global options shown above —
run `byteshaver <command> --help` for the full list with descriptions):

| command | command-specific options |
|---------|--------------------------|
| `webp` | `--lossless` · `-q, --quality <0-100>` (default 90) |
| `webp-image` | — |
| `avif` | `-q, --quality <0-100>` (default 90) · `-s, --speed <1-10>` (default 3) · `--bit-depth eight\|ten\|auto` · `--color-model y-cb-cr\|rgb` · `--alpha-color-mode unassociated-dirty\|unassociated-clean\|premultiplied` · `-a, --alpha-quality <0-100>` |
| `png` | `--compression-type default\|fast\|best` · `--filter-type no-filter\|sub\|up\|avg\|paeth\|adaptive` |
| `jpeg` | — |
| `jxl` | `-q, --quality <0-100>` · `--distance <0-25>` (mutually exclusive with `--quality`) · `--lossless` · `-e, --effort <1-10>` (default 7) · `--container` · `--original-profile` · `--decoding-speed <0-4>` · `--intensity-target <nits>` · `--bit-depth 8\|16` · `--color-encoding srgb\|linear-srgb\|srgb-luma\|linear-srgb-luma\|icc-passthrough` · `--setting <ID=VALUE>` (repeatable) |
| `oxipng` | `-l, --level <0-6\|max>` (default 2) · `--zopfli` · `--zopfli-iterations <n>` (default 15) · `--interlace keep\|none\|adam7` · `--strip none\|safe\|all` · `--filters <list>` · `--optimize-alpha` · `--no-reduction <list>` · `--scale-16` · `--fix-errors` · `--timeout-secs <s>` |
| `webp-anim` | `--lossless` · `-q, --quality <0-100>` (default 90) · `--kmin <n>` · `--kmax <n>` · `--minimize-size` · `--allow-mixed` · `--method <0-6>` |
| `apng` | `--compression-type default\|fast\|best` · `--filter-type no-filter\|sub\|up\|avg\|paeth\|adaptive` |
| `gif` | `-s, --speed <1-10>` |
| `clean` | — |

---

## Examples

### Input Directory Structure

```bash
examples
├── 1.png
├── 1.webp
├── img1
│   ├── 2.png
│   ├── 2.webp
│   └── img11
│       ├── 3.jpg
│       └── 3.webp
├── img2
│   ├── 4.jpeg
│   └── 4.webp
...
```

Example of webp command:

![Webp command example](/docs/img/webp_cmd.webp)

Example of clean command:

![Clean command example](/docs/img/clean_cmd.webp)

---

## Building from source

### Prerequisites

- Ensure you have the latest stable version of `Rust` and `Cargo` installed on your system.
- [Nasm](https://www.nasm.us/) is needed for building `rav1e`.
  Install via `apt install nasm` / `apk add nasm` / `choco install nasm`.
- `cmake`, a C++ compiler and `nasm` are needed for building the vendored `libjxl`
  (jpeg-xl support; enabled by default via the `jxl` feature).
  Install via `apt install cmake g++ nasm` / `apk add cmake g++ nasm`.
- The opt-in `dec-heif` feature (HEIC/HEIF/AVIF input) needs the native
  `libheif` + codec development libraries at build time
  (`apt install libheif-dev libde265-dev libaom-dev pkg-config` / `apk add libheif-dev libde265-dev aom-dev`).

### Installation Guide

#### Install via crate

To install via the [published crate](https://crates.io/crates/byteshaver), execute the following command:

```bash
cargo install byteshaver
```

#### Install from git

```bash
# 1. Clone the repository:
git clone https://github.com/Gunzinger/byteshaver.git
cd byteshaver
# 2. Build the project:
cargo build --release
# 3. Install locally
cargo install --path .
```

#### Uninstalling

To uninstall, remove the tool via `cargo uninstall`:

```bash
cargo uninstall byteshaver
```

---

## What's Next

- [x] Testing (unit + integration suite; `cargo test --workspace`)
- [x] Publishing automation (binaries for CLI + GUI, docker incl. libheif validation)
- [ ] Introduce advanced options for image transformations (resize, rotate)
- [x] Progress bar for encoding
- [ ] Expand support for additional input formats 
  - [x] `avif` (via libheif)
  - [x] `png`
  - [x] `jpeg` (incl. progressive/pjpeg fallback decoding)
  - [x] `heic/heif` (input, via libheif / `dec-heif` feature; enabled in docker images, stubbed in musl/windows release binaries for now)
  - [x] `jxl/jpeg-xl` (via jxl-oxide; stills + animation + metadata)
  - [ ] `heic` in static release binaries (needs a statically linkable libheif + codec stack in CI)
  - [ ] animated `avif` input
  - [ ] incoming wishes
- [x] Expand support for additional export formats by including more encoding libraries
  - [x] `jxl` (libjxl, full option surface)
  - [x] `oxipng` (lossless PNG re-optimization)
  - [x] animated `webp` / `apng` / `gif`
  - [ ] animated `avif` (blocked upstream: needs libheif ≥ 1.20 with an AV1 encoder; see `docs/plans/06-*`)
- [x] Image metadata handling (EXIF data preservation/stripping/filtering)
  - [x] `avif` EXIF embedding (native Exif item via ravif >= 0.13)
- [x] Output logs (JSON-lines event log via `--json-log`)
- [ ] `winresource` integration (application icon and .exe metadata for Windows binaries)
- [x] GUI (egui/eframe desktop front-end; drag-and-drop queue; see `gui/`)
  - [ ] GUI packaging polish (AppImage/.app/NSIS installers, icons, file associations)

---

## License

This project under the [MIT License](LICENCE).
