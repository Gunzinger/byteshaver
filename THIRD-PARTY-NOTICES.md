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
