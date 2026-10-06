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

## 6. Startup latency vs unpacked and UPX

`packer-out/bench-start.exe` times CreateProcess→exit with QueryPerformanceCounter
(child stdio goes to NUL, so console output costs nothing) and reports
min/p50/p90/mean/max. Run it against the unpacked, zpe-packed and UPX-packed
CLI using `--version` as the minimal workload.

```sh
# one-time: get upx.exe (windows build) from https://github.com/upx/upx/releases
mkdir -p /mnt/c/Users/<you>/bench
cd ~/development/byteshaver
cp packer-out/bench-start.exe byteshaver-packed.exe /mnt/c/Users/<you>/bench/
cp target/x86_64-pc-windows-gnullvm/release/byteshaver.exe /mnt/c/Users/<you>/bench/byteshaver-unpacked.exe
/mnt/c/Users/<you>/Downloads/upx-<ver>-win64/upx.exe --best -o \
    /mnt/c/Users/<you>/bench/byteshaver-upx.exe \
    /mnt/c/Users/<you>/bench/byteshaver-unpacked.exe

cmd.exe /c 'cd /d %USERPROFILE%\bench && bench-start.exe -n 30 -w 5 byteshaver-unpacked.exe --version'
cmd.exe /c 'cd /d %USERPROFILE%\bench && bench-start.exe -n 30 -w 5 byteshaver-packed.exe --version'
cmd.exe /c 'cd /d %USERPROFILE%\bench && bench-start.exe -n 30 -w 5 byteshaver-upx.exe --version'
```

Methodology notes:

- **Run from a native Windows folder** (`C:\Users\...`), never from `\\wsl$` —
  9p file access inflates the image-load time of every variant equally but
  adds jitter. The harness itself may be launched through interop; the timed
  child processes run natively.
- **Warm numbers are the comparable metric** (the harness warms up first);
  cold start is dominated by disk cache and is not packer-specific.
- **Defender**: real-time protection scans every exe launch, and the zpe
  stub additionally writes+launches a fresh temp exe each run (extra scan).
  For microbenchmarks, exclude the bench folder — then re-run once without
  the exclusion for real-world numbers. UPX/packed exes may also get
  flagged heuristically; that is part of the honest comparison.
- The unpacked variant is the floor; the delta over it is the packer cost.
  bench-start prints a WARNING when the child exits non-zero (a packed stub
  failing via die() exits 127 and would silently produce fast garbage).
- GUI startup-to-window is not automatically measurable this way; compare
  CLI `--version` and judge the GUI by feel.

## 7. Measured baseline (real Windows, warm cache)

First measured matrix (NVMe, warm, Defender real-time protection ON):

| variant          | min    | p50    | p90    | mean   | delta vs floor |
|------------------|--------:|-------:|-------:|-------:|---------------:|
| unpacked         | 12.83  | 13.42  | 14.09  | 13.43  | —              |
| UPX `--best`     | 68.42  | 69.36  | 70.81  | 69.56  | +56 ms         |
| zpe v3           | 93.85  | 96.98  | 99.71  | 97.36  | +84 ms         |

Phase breakdown — the stub logs QPC-timed milestones with `ZPE_DEBUG=1`:

```powershell
$env:ZPE_DEBUG = "1"; .\byteshaver-packed.exe --version; Remove-Item Env:ZPE_DEBUG
Get-Content $env:TEMP\zpe-debug.log -Tail 5
# zpe: +N ms self read          <- read packed exe (grows with size)
# zpe: +N ms decompressed       <- parallel zstd decompress
# zpe: +N ms payload written    <- 16 MB temp-file write
# zpe: +N ms done               # done minus written = CreateProcess + PE
#                               #   loader + Defender scan of the fresh exe
```

The `done − payload written` span contains the loader for the freshly
written exe **and** the Defender scan it triggers. Measured split (Defender
excluded for the bench folder, `%TEMP%` still scanned):

```
zpe: +1 ms self read          <- stub-side total: 15 ms
zpe: +7 ms decompressed          (read 1 + parallel decompress 6 + write 8)
zpe: +15 ms payload written
zpe: +61 ms done              <- CreateProcess + loader + AV on the fresh exe
```

**Extract-cache** (implemented since): the stub caches the extracted exe at
`%LOCALAPPDATA%\byteshaver\zpe-cache\<key>\<packed-name>.exe` (hash in the
directory, real binary name preserved for Task Manager) and launches it
directly on
subsequent runs (`ZPE_NO_CACHE=1` restores the temp behavior). Measured on
real Windows (`ZPE_DEBUG=1`):

```
first run (miss):  +2 ms self read / +7 ms decompressed / +30 ms payload
                   written / +72 ms done
warm run (hit):    +0 ms cache hit / +7 ms done   <- total, incl. the app
```

Warm startup is therefore at parity with the unpacked binary and ~10x
faster than UPX. A repacked binary gets a new key (FNV-1a over the frame
table + payload head) and evicts the previous generation.

**GUI payloads detach**: the stub reads its own (patched) Subsystem field —
for GUI payloads it launches the cached exe and exits immediately
(fire-and-forget), so only the app process remains visible; there is no
lingering parent. Console payloads still wait and propagate the exit code.

## CI parity

`.github/workflows/workflow.yaml` ("Build windows binaries (cross)") runs the
same toolchain and env as `env-win.sh` (llvm-mingw 20260922, ucrt, static
libc++/libunwind); `tools/libheif-static/build-decode-only-windows.sh` builds
the codec deps; release artifacts are shipped unpacked — packing is for
local/experimental distribution until release-channel AV scanning is done.
