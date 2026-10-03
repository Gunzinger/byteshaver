# Third-party notices

Binaries produced by this repository's pipeline (including the optional
`*-packed` variants built with the packer in `tools/packer/`) statically
include code from the following projects. This file satisfies the
attribution conditions of their licenses.

## libheif + libde265 (dec-heif feature, statically embedded)

`dec-heif` statically embeds **libheif** (via `libheif-sys`'s
`embedded-libheif` feature, vendored at 1.23.1) and statically links
**libde265** (built by `tools/libheif-static/build-decode-only.sh`).
Both are distributed under the **GNU Lesser General Public License,
version 3.0 (LGPL-3.0)**.

Statically linking LGPL-3.0 libraries creates a Combined Work. To satisfy
LGPL-3.0 §4 (Installation Information / Corresp. Source) for the release
binaries, this repository:

- pins the exact upstream sources used (libheif `1.23.1` via the vendored
  copy in `libheif-sys 5.3.1`; libde265 `1.1.3` via the pinned tarball URL in
  the build recipe), and
- publishes the full build recipe (`tools/libheif-static/`) that relinks the
  application against modified versions of these libraries — rebuilding from
  the pinned sources with that script and re-linking reproduces the
  combined work.

The libheif/libde265 sources remain under their original LGPL-3.0 terms;
nothing in this repository modifies them.

Upstream: https://github.com/strukturag/libheif (LGPL-3.0),
https://github.com/strukturag/libde265 (LGPL-3.0).

## dav1d (dec-heif feature, statically linked)

AV1 decoder used for AVIF input, distributed under the **BSD-2-Clause
license** (Copyright © VideoLAN and dav1d authors):

```
Copyright © 2018-2021, VideoLAN
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

Source: https://code.videolan.org/videolan/dav1d (pinned release 1.5.1)

## Zstandard (libzstd, decompression only)

Used by the self-extracting packer stubs (`tools/packer/zpack_stub.c`,
`tools/packer/zpe_stub.c`), statically linked in its decompression-only
configuration. We use and distribute it under the BSD-2-Clause license
below (the project offers a choice of BSD-2-Clause or GPLv2+).
The bundled xxhash implementation carries the same dual license.

```
BSD License

For Zstandard software

Copyright (c) Meta Platforms, Inc. and affiliates. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

 * Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

 * Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

 * Neither the name Facebook, nor Meta, nor the names of its contributors may
   be used to endorse or promote products derived from this software without
   specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

Source: https://github.com/facebook/zstd (release 1.5.7)

## libjxl + bundled third-party code (jxl feature, statically embedded)

The JPEG XL encoder/decoder is statically embedded via `jpegxl-src`'s vendored
libjxl, together with its bundled third-party components. All are distributed
under permissive licenses; copyright and license files are retained in the
vendored sources:

- **libjxl** — Apache-2.0 ("Copyright (c) the libjxl authors")
- **highway** (SIMD dispatch) — Apache-2.0 (Google LLC)
- **brotli** (encoder/decoder, statically linked) — MIT ("Copyright (c) the Brotli Authors")
- **skcms** (color management) — Apache-2.0 (Google LLC)
- **zlib** (bundled) — Zlib license (Jean-loup Gailly, Mark Adler)

Source: https://github.com/libjxl/libjxl (pinned by `tools/jpegxl-src`,
see `tools/patches/`).

Apache-2.0 components additionally carry the following notice text:

```
Copyright <year> <copyright holders>

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

## LLVM runtime libraries (Windows gnullvm binaries)

The Windows binaries built on the `x86_64-pc-windows-gnullvm` target
statically include **libc++**, **libc++abi**, **libunwind** and
**compiler-rt builtins** from the LLVM project (shipped by the llvm-mingw
toolchain), distributed under
**Apache-2.0 WITH LLVM-exception** (© the LLVM Project, see above Apache
text; the full exception text ships in the toolchain's license files and at
https://llvm.org/docs/DeveloperPolicy.html#new-llvm-license-generator).

Source: https://github.com/llvm/llvm-project via
https://github.com/mstorsjo/llvm-mingw

## mozjpeg / libjpeg-turbo (jpeg feature, statically embedded)

The JPEG encoder/optimizer statically embeds mozjpeg (libjpeg-turbo based),
distributed under the **IJG license** (the original Independent JPEG Group
license, including the requirement to acknowledge use of this software "in
materials provided with the distribution" — satisfied by this file) combined
with **BSD-3-Clause** (libjpeg-turbo portions) and the **Zlib** license:

```
The authors make NO WARRANTY or representation, either express or implied,
with respect to this software, its quality, accuracy, merchantability, or
fitness for a particular purpose. This software is provided "AS IS" ...

Permission is hereby granted to use, copy, modify, and distribute this
software (or portions thereof) for any purpose, without fee, subject to
these conditions:
(1) ... acknowledge ... this software is based in part on the work of the
Independent JPEG Group.
...
```

Full texts ship in the mozjpeg source
(https://github.com/mozilla/mozjpeg — IJG license, BSD-3-Clause, README-turbo).

## libwebp (webp feature, statically embedded)

Statically linked via `libwebp-sys2`'s bundled build — **BSD-3-Clause**
(© Google Inc., "Copyright (c) the WebM Project authors").

Source: https://github.com/webmproject/libwebp

## GUI fonts (byteshaver-gui)

The GUI statically embeds the default fonts bundled by egui: **Ubuntu-Light**
(Ubuntu Font Licence 1.0), **Noto Emoji** and **emoji-icon-font**
(SIL Open Font License 1.1 / permissive font licenses). See the egui
repository for the embedded font license files.
https://github.com/emilk/egui

## Dependency inventory (Rust crates)

All ~210 crates distributed in the release binaries are permissive
(MIT / Apache-2.0 / BSD / Zlib / CC0 / Unicode families) — the complete
per-crate list with versions and SPDX expressions is generated by
`tools/license-inventory.sh` into
[docs/dependency-licenses.md](docs/dependency-licenses.md) and is attached
to each release. No copyleft crate is linked into the binaries.

## mingw-w64 runtime (Windows stubs and Windows binaries)

The Windows stubs and the Windows binaries produced by the cross toolchain
statically include portions of the mingw-w64 runtime, whose components are
distributed under permissive licenses (see
https://sourceforge.net/p/mingw-w64/mingw-w64/ci/master/tree/). This is the
same runtime already included in the project's regular Windows binaries.

## Notes on CI-only tools

- The `zstd` command-line tool (GPLv3+) and `wine` (LGPL) are used inside CI
  jobs only and are never distributed with, or embedded in, any artifact;
  their licenses impose no conditions on the produced binaries.
- UPX is no longer used by the pipeline.
