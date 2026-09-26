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
Using "ravif" (0.12.0) with options (quality: 90, speed: 3, bit depth: Eight, color model: RGB)
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
 Works with many [supported image formats](#supported-formats).
- **Works with huge images**:
  Can optimize very large images (~1GiB input image size, ~32Kx~16K px dimensions).
- **Speedy Processing**:
  Written in Rust to keep overhead to a minimum, we also take advantage of `rayon` for parallel processing.
- **Input selection using Glob Patterns**:
  Target selection is made intuitive for cli enthusiasts via glob patterns.
- **Custom Output**:
 Choose where your converted images are saved.

---

## Supported formats

### Input formats 🖼️

To keep it simple: `JPEG`, `PNG`, `GIF`, `WebP`, `JPEG XL`, `BMP`, `DDS`, `Farbfeld`, `HDR`, `ICO`, `EXR`, `PNM`, `QOI`, `TGA`, `TIFF`

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
rest of the batch keeps converting. Please note that the **release musl and windows
binaries are built without** `dec-heif` (the native libheif + codec libraries cannot be
bundled there yet), while the **docker images include it**.

`JPEG XL` (`.jxl`) inputs are decoded with [`jxl-oxide`](https://crates.io/crates/jxl-oxide)
 (still images and animations, 8/16-bit, EXIF/XMP boxes and ICC profiles).
 Requires the `jxl` feature (enabled by default).

### Output formats 📤

- `webp`, webp encoder using the `webp` crate (libwebp bindings) - offers lossy and lossless encoding
- `webp-image`, webp encoder using the `image` crate - offers lossless encoding
- `avif`, avif encoder using the `ravif` crate - offers lossy and lossless encoding
- `png`, png encoder using the `image` crate - offers lossless encoding
- `jpeg`, jpeg optimizer using the `mozjpeg` crate - only optimizes images
- `jxl`, jpeg-xl encoder using in-tree FFI bindings to `libjxl` (vendored static build via `jpegxl-src`) - offers
  lossy and lossless encoding, real animated output (frame durations), EXIF embedding and the full
  `JxlEncoderFrameSettingId` surface via repeatable `--setting ID=VALUE` flags. Requires the `jxl` feature
  (enabled by default) and `cmake`, a C++ compiler and `nasm` at build time.

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

### Animations 🎞️

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

### Requests

If this does not cover your needs,
 please feel free to open an issue to request additional input and/or output formats.

I am focusing on supporting modern image formats supported in browsers,
 as this tool is optimally suited for optimizing static directories for different web apps.

For a good overview of browser support, see the [caniuse.com](https://caniuse.com) pages
 for different images, e.g.: [avif](https://caniuse.com/avif), [webp](https://caniuse.com/webp).

---

## Installation 💾

### Using published binaries 📡

Binaries for Windows and Linux are built for every tag.

See the [GitHub releases](https://github.com/Gunzinger/byteshaver/releases) page for downloads.

### Using the docker image 🐳

Docker containers are also built for every tag.

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
  clean       Remove files matching a glob pattern
  help        Print this message or the help of the given subcommand(s)

Arguments:
  <PATTERN>  Glob pattern to match images to convert. Example: `images/**/*.png`

Options:
  -o, --output <OUTPUT>               Output directory (flat) of processed images. Defaults to the same location as the original images with the new file extension. If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
      --reverse-processing-order      By default, byteshaver will process input files in lexicographical order after expanding the pattern. Setting this starts the process from the back
      --overwrite-if-smaller          Overwrite the existing output file if the current conversion resulted in a smaller file
      --overwrite-existing            Overwrite existing output files regardless of size
      --discard-if-larger-than-input  Discards the encoding result if it is larger than the input file (does not create an output file)
      --discard-input-alpha-channel   Discards the alpha channel of the input image(s) if it is present. (this does not make loading faster, but it can improve the encoding result)
  -h, --help                          Print help
  -V, --version                       Print version
```

For the `webp` command:

```bash
❯ byteshaver webp --help
Convert images to webp format (using webp crate)

Usage: byteshaver <PATTERN> webp [OPTIONS]

Options:
      --lossless                      Use lossless encoding mode. Defaults to false
  -q, --quality <QUALITY>             Control target quality (0 - 100, lower is worse but results in smaller files). Defaults to 90.0
  -o, --output <OUTPUT>               Output directory (flat) of processed images. Defaults to the same location as the original images with the new file extension. If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
      --reverse-processing-order      By default, byteshaver will process input files in lexicographical order after expanding the pattern. Setting this starts the process from the back
      --overwrite-if-smaller          Overwrite the existing output file if the current conversion resulted in a smaller file
      --overwrite-existing            Overwrite existing output files regardless of size
      --discard-if-larger-than-input  Discards the encoding result if it is larger than the input file (does not create an output file)
      --discard-input-alpha-channel   Discards the alpha channel of the input image(s) if it is present. (this does not make loading faster, but it can improve the encoding result)
  -h, --help                          Print help
```

For the `webp-image` command:

```bash
❯ byteshaver webp-image --help
Convert images to webp format (using image crate)

Usage: byteshaver <PATTERN> webp-image [OPTIONS]

Options:
  -o, --output <OUTPUT>               Output directory (flat) of processed images. Defaults to the same location as the original images with the new file extension. If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
      --reverse-processing-order      By default, byteshaver will process input files in lexicographical order after expanding the pattern. Setting this starts the process from the back
      --overwrite-if-smaller          Overwrite the existing output file if the current conversion resulted in a smaller file
      --overwrite-existing            Overwrite existing output files regardless of size
      --discard-if-larger-than-input  Discards the encoding result if it is larger than the input file (does not create an output file)
      --discard-input-alpha-channel   Discards the alpha channel of the input image(s) if it is present. (this does not make loading faster, but it can improve the encoding result)
  -h, --help                          Print help
```

For the `avif` command:

```bash
❯ byteshaver avif --help
Convert images to avif format (using ravif crate)

Usage: byteshaver <PATTERN> avif [OPTIONS]

Options:
  -q, --quality <QUALITY>
          Control target quality (0 - 100, lower is worse but results in smaller files). Defaults to 90.0
  -s, --speed <SPEED>
          Control encoding speed (1 - 10, lower is much slower but has a better quality and lower filesize). Defaults to 3
      --bit-depth <BIT_DEPTH>
          Choose internal bit depth. (in the generated avif file, nothing to do with the input file) [possible values: eight, ten, auto]
      --color-model <COLOR_MODEL>
          Choose internal color model. (in the generated avif file, nothing to do with the input file) [possible values: y-cb-cr, rgb]
      --alpha-color-mode <ALPHA_COLOR_MODE>
          Choose internal alpha color mode. (in the generated avif file, nothing to do with the input file) Irrelevant for images without transparency [possible values: unassociated-dirty, unassociated-clean, premultiplied]
  -a, --alpha-quality <ALPHA_QUALITY>
          Control target alpha quality (0 - 100, lower is worse). Defaults to 90.0
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
  -h, --help
          Print help
```

For the `png` command:

```bash
❯ byteshaver png --help
Convert images to png format (using image crate)

Usage: byteshaver <PATTERN> png [OPTIONS]

Options:
      --compression-type <COMPRESSION_TYPE>
          Choose the png compression type
          
          See: https://docs.rs/image/latest/image/codecs/png/enum.CompressionType.html
          
          [possible values: default, fast, best]

      --filter-type <FILTER_TYPE>
          Choose the png filter type
          
          See: https://docs.rs/image/latest/image/codecs/png/enum.CompressionType.html
          
          [possible values: no-filter, sub, up, avg, paeth, adaptive]

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

  -h, --help
          Print help (see a summary with '-h')
```

For the `jpeg` command (unstable; likes to crash! this is a work in progress!):

```bash
❯ byteshaver jpeg --help
Convert images to optimized jpeg format (using mozjpeg crate)

Usage: byteshaver <PATTERN> jpeg [OPTIONS]

Options:
  -o, --output <OUTPUT>               Output directory (flat) of processed images. Defaults to the same location as the original images with the new file extension. If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
      --reverse-processing-order      By default, byteshaver will process input files in lexicographical order after expanding the pattern. Setting this starts the process from the back
      --overwrite-if-smaller          Overwrite the existing output file if the current conversion resulted in a smaller file
      --overwrite-existing            Overwrite existing output files regardless of size
      --discard-if-larger-than-input  Discards the encoding result if it is larger than the input file (does not create an output file)
      --discard-input-alpha-channel   Discards the alpha channel of the input image(s) if it is present. (this does not make loading faster, but it can improve the encoding result)
  -h, --help                          Print help
```

For the `jxl` command:

```bash
❯ byteshaver jxl --help
Convert images to jpeg-xl format (using libjxl)

Usage: byteshaver <PATTERN> jxl [OPTIONS]

Options:
  -q, --quality <QUALITY>             JPEG-style quality 0-100 (higher = better). Mutually exclusive with --distance
      --distance <DISTANCE>           Maximum Butteraugli distance 0.0-25.0 (0.0 = mathematically lossless, 1.0 = visually lossless, libjxl default 1.0). Mutually exclusive with --quality
      --lossless                      Lossless mode. Overrides quality/distance
  -e, --effort <EFFORT>               Encoding effort 1 (fastest) - 10 (slowest/best). Defaults to 7
      --container                     Force the box-based container format (required for manual Exif/XMP embedding; auto-enabled when EXIF is embedded)
      --original-profile              Keep the original color profile (do not convert to internal XYB); needed for lossless
      --decoding-speed <DECODING_SPEED>  Target decode speed tier 0-4 (higher = faster decode, larger file). Defaults to 0
      --intensity-target <INTENSITY_TARGET>  Photometric target intensity in nits (HDR). Defaults to libjxl's 255
      --bit-depth <BIT_DEPTH>         Force output bit depth: 8 or 16 (default: follow the input)
      --color-encoding <COLOR_ENCODING>  Color encoding: srgb | linear-srgb | srgb-luma | linear-srgb-luma | icc-passthrough. Defaults to srgb
      --setting <ID=VALUE>            Advanced: repeatable libjxl frame-setting passthrough, e.g. --setting brotli_effort=9 (ids are resolved case-insensitively; unknown ids list the available set)
  -o, --output <OUTPUT>               Output directory (flat) of processed images. Defaults to the same location as the original images with the new file extension. If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
      --reverse-processing-order      By default, byteshaver will process input files in lexicographical order after expanding the pattern. Setting this starts the process from the back
      --overwrite-if-smaller          Overwrite the existing output file if the current conversion resulted in a smaller file
      --overwrite-existing            Overwrite existing output files regardless of size
      --discard-if-larger-than-input  Discards the encoding result if it is larger than the input file (does not create an output file)
      --discard-input-alpha-channel   Discards the alpha channel of the input image(s) if it is present. (this does not make loading faster, but it can improve the encoding result)
  -h, --help                          Print help

```

For the `clean` command:

```bash
❯ byteshaver clean --help
Remove files matching a glob pattern

Usage: byteshaver <PATTERN> clean [OPTIONS]

Options:
  -o, --output <OUTPUT>               Output directory (flat) of processed images. Defaults to the same location as the original images with the new file extension. If set, replaces the fixed base of the pattern directory structure of the input pattern. (before any * in the glob pattern)
      --reverse-processing-order      By default, byteshaver will process input files in lexicographical order after expanding the pattern. Setting this starts the process from the back
      --overwrite-if-smaller          Overwrite the existing output file if the current conversion resulted in a smaller file
      --overwrite-existing            Overwrite existing output files regardless of size
      --discard-if-larger-than-input  Discards the encoding result if it is larger than the input file (does not create an output file)
      --discard-input-alpha-channel   Discards the alpha channel of the input image(s) if it is present. (this does not make loading faster, but it can improve the encoding result)
  -h, --help                          Print help

```

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
3. Install locally
cargo install --path .
```

#### Uninstalling

To uninstall, remove the tool via `cargo uninstall`:

```bash
cargo uninstall byteshaver
```

---

## What's Next

- [ ] Testing
- [x] Publishing automation (binaries, docker)
- [ ] Introduce advanced options for image transformations (resize, rotate)
- [x] Progress bar for encoding
- [ ] Expand support for additional input formats 
  - [x] `avif`
  - [x] `png`
  - [ ] `jpeg` (WIP)
  - [ ] `png` (via `oxipng` crate)
  - [x] `heic/heif` (input, via libheif / `dec-heif` feature; enabled in docker images, stubbed in musl/windows release binaries for now)
  - [ ] `jxl/jpeg-xl`
  - [ ] incoming wishes
- [ ] Expand support for additional export formats by including more encoding libraries
- [ ] Image metadata handling (EXIF data preservation/stripping)
- [ ] Expand support for animated images/video encoding (to webp/avif/apng)
- [ ] Output logs (to enable usage in automations static directory optimizations by link-rewriting)
- [ ] `winresource` integration (application icon and .exe metadata for Windows binaries)
- [ ] GUI

---

## License

This project under the [MIT License](LICENCE).
