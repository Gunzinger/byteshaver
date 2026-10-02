# Third-party notices

Binaries produced by this repository's pipeline (including the optional
`*-packed` variants built with the packer in `tools/packer/`) statically
include code from the following projects. This file satisfies the
attribution conditions of their licenses.

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
