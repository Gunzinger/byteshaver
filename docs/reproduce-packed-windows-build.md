# Reproducing the packed Windows build (zpe v3)

End-to-end commands to build, pack and test the self-extracting Windows
binaries. Everything runs in WSL2 (Ubuntu 24.04); the packed exes are
executed on the real Windows host via WSL interop. The Linux ELF flow is
analogous (`zpack.py` without `--pe`, stub `zpack_stub.c`).

Status: validated 2026-10 — wine (sandbox) byte-exact extraction, exit-code
propagation, corrupt-payload die(127); real-Windows interop re-verified per
build by step 5.

## 1. One-time setup

```sh
cd ~/development/byteshaver
tools/packer/setup-wsl.sh     # llvm-mingw, rust target, static libde265+dav1d, env-win.sh
. ./env-win.sh                # per-shell: linker, PKG_CONFIG_PATH, cross CC/CXX/AR
tools/packer/selftest.sh      # must end: 0 failed
```

## 2. Build the unpacked exes

```sh
cargo build --release --target x86_64-pc-windows-gnullvm -p byteshaver
cargo build --release --target x86_64-pc-windows-gnullvm -p byteshaver-gui --no-default-features
# self-containment guard (must print nothing):
for exe in target/x86_64-pc-windows-gnullvm/release/byteshaver{,-gui}.exe; do
  objdump -p "$exe" | grep -i 'DLL Name: lib\(stdc++\|c++\|unwind\|gcc_s\|winpthread\)' && exit 1
done
```

## 3. Build the packer stub + pack

```sh
tools/packer/build-stubs.sh   # -> packer-out/zpe-stub.exe (and zpack-stub for ELF)
python3 tools/packer/zpack.py packer-out/zpe-stub.exe \
    target/x86_64-pc-windows-gnullvm/release/byteshaver.exe \
    byteshaver-packed.exe --pe
python3 tools/packer/zpack.py packer-out/zpe-stub.exe \
    target/x86_64-pc-windows-gnullvm/release/byteshaver-gui.exe \
    byteshaver-gui-packed.exe --pe
```

`--pe-chunk 4` (default) balances frame sizes for the parallel decompressor;
`--level` defaults to 19. The stub's subsystem is patched to the payload's
(GUI payload => no extra console window).

## 4. Sanity-test the packer itself (no wine needed)

```sh
./packer-out/selftest-packed.exe alpha "two words"   # ARG:alpha / ARG:two words / ARGS:2
./packer-out/selftest-packed.exe 42; echo $?         # 42
```

(runs on the Windows host through WSL interop; output and exit code flow
back through the stub's std-handle forwarding)

## 5. Test the real packed binaries

```sh
./byteshaver-packed.exe --version; echo $?           # version string, 0
./byteshaver-packed.exe <image.heic> <out.jpg>       # real HEIC/AVIF decode path
ZPE_DEBUG=1 ./byteshaver-gui-packed.exe              # GUI window must appear
```

## Diagnostics

| symptom | meaning / action |
|---|---|
| `$?` = 5 | low byte of 0xC0000005 (access violation) — run with `ZPE_DEBUG=1` |
| `$?` = 127 | stub hit its die() path (bad trailer/frames, decompress, CreateProcess) |
| error MessageBox | stub failure, GUI-visible by design; suppressed when `ZPE_DEBUG=1` |
| no output when piped | should not happen — child inherits std handles; report as bug |

- `ZPE_DEBUG=1` — appends each phase to `%TEMP%\zpe-debug.log`
  (`C:\Users\<you>\AppData\Local\Temp\zpe-debug.log`); headless-safe.
- `ZPE_KEEP_TEMP=1` — keeps the extracted exe in `%TEMP%` (`zpe-<pid>.exe`);
  compare `sha256sum` against the unpacked binary to prove extraction is
  byte-exact.
- `ZPE_THREADS=1` — decompress serially (benchmarking).

## CI parity

`.github/workflows/workflow.yaml` ("Build windows binaries (cross)") runs the
same toolchain and env as `env-win.sh` (llvm-mingw 20260922, ucrt, static
libc++/libunwind); `tools/libheif-static/build-decode-only-windows.sh` builds
the codec deps; release artifacts are shipped unpacked — packing is for
local/experimental distribution until release-channel AV scanning is done.
