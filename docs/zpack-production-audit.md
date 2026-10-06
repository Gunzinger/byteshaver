# zpack production-readiness audit

Date: 2026-10-04 · Scope: `tools/packer/` (`zpack.py`, `zpack_stub.c`, `zpe_stub.c`,
`build-stubs.sh`, `selftest.sh`, `bench-start.c`), the `pack_binaries` CI job, and
the zstd-1.5.7 vendoring/build path. Method: full code review plus empirical
testing of the ELF path on this machine (built stub against the vendored zstd,
round-trip, corruption matrix, page-size variants, packing-time benchmarks).
The PE stub could not be executed here (no mingw/wine on this box) — PE findings
are code-review based; each is tagged `[code]`.

**Overall verdict: sound architecture, well-measured design — not yet
production-hardened.** Three issues should be fixed before shipping (one
correctness UB in the PE stub, one real-world cache race, one supply-chain gap),
plus one robustness regression against the project's own stated guarantee
(SIGBUS instead of clean exit 127 on table corruption). All fixes are small
(hours, not days). Everything else is polish.

---

## 0. What is already right (kept as-is)

- Atomic work-queue decompression + per-frame `madvise(MADV_DONTNEED)` (ELF) —
  validated design, good RSS/startup numbers.
- zstd frames carry content checksums (CLI default) → in-stub integrity.
  Verified: byte-flip inside frame data → clean `zpk: a frame failed`, exit 127.
- `.tmp` + `MoveFileEx(REPLACE)` cache commit; FNV-1a generation keys;
  eviction of stale generations.
- `MFD_EXEC` fallback for pre-6.3 kernels; exit-code/argv/env propagation;
  GUI-subsystem passthrough + detach.
- Honest docs: negative results recorded (streaming input §7.6, in-memory PE
  loading §10.3), licensing analyzed, limitations listed.
- Selftest round-trips ELF + PE (wine), including a corruption leg.

## 1. Correctness findings

Severity: **HIGH** = fix before production · **MED** = fix soon · **LOW** = nit.

### C1. HIGH [code] — PE stub: compressed lengths are never bounds-checked (heap OOB read)

`zpe_stub.c:191-205` (`decompress_payload`): `src = payload_off;
jobs[i].src = g_self + src; src += comp` — the per-frame `comp` values from
the trailer are used without validating `sum(comp) ≤ self_len − 24 − nframes*24`
(the trailer check at `zpe_stub.c:347` only constrains `payload_off` and
`nframes`). A corrupted/malicious table entry (e.g. `comp = 2^60`) makes
`ZSTD_decompress` read far out of bounds of the 16 MB `g_self` heap buffer.
The `dest`/`uncomp` pair *is* validated (`zpe_stub.c:194`) — the `comp` side is
the gap. Disk corruption or a truncated download turns into silent UB instead
of a clean `die(127)`.

**Fix:** before spawning workers, iterate the table once:
`if (src_off + comp > g_self_len - 24 - nframes*24) die("zpe: bad frame table")`.
~6 lines, mirrors the existing dest check.

### C2. HIGH [code] — PE stub: concurrent first run can poison the cache permanently

Two processes launched simultaneously on a fresh install (double-click +
terminal, or two terminals) both take the cache-miss path and both write the
**same** `<key>.tmp` (`zpe_stub.c:220-222`, `CREATE_ALWAYS`). Process B's
`CREATE_ALWAYS` truncates the file while A is mid-write; whichever finishes
first `MoveFileEx`es a **torn payload into the cache as `<key>.exe`**. The bad
entry is never invalidated (the key depends only on the packed file, which is
fine) — every subsequent launch runs a corrupt binary until the file is
repacked or the cache is cleared by hand. The no-cache temp path already uses
per-PID names (`zpe-%lu.exe`, `zpe_stub.c:401`) — the cache write should too.

**Fix:** tmp name `<key>\<base>.<pid>.tmp` (pid already available via
`GetCurrentProcessId`); delete-on-rename-failure; optionally tolerate leftover
tmp files in `cache_evict_others` (it already deletes files inside foreign
generation dirs, so housekeeping is covered).

### C3. MED (verified) — ELF stub: corrupt `comp_len` → SIGBUS, violating the clean-127 guarantee

`zpack_stub.c:118-119`: `payload_len = Σ comp` is computed from the untrusted
table and passed straight to `mmap`. A huge `comp` value maps successfully
(overcommit), and `ZSTD_decompress` touching pages past the file's last page
raises **SIGBUS** (core dump) instead of `die(127)`. Verified on this machine:

| corruption (packed4) | result |
|---|---|
| byte flip inside frame 0 (selftest's case) | clean 127 `zpk: a frame failed` ✓ |
| `uncomp_len` of frame 0 → garbage | clean 127 ✓ (zstd length check) |
| `comp_len` of frame 0 → 2^40 | **SIGBUS, rc=135, core dumped** ✗ |
| `payload_off` → past EOF | clean 127 `mmap payload` ✓ |

`docs/zstd-packer-analysis.md` §7.4 and the README promise "corrupted payload
→ clean exit 127, no crash"; that holds for frame data but not for the frame
table. A crash-looping packed binary (e.g. after bad disk sectors) is a bad
failure mode for a released artifact.

**Fix:** validate `payload_off + payload_len ≤ fsz - 24 - tbl_sz` before
`mmap` (and optionally per-frame `so + comp ≤ payload_len` to catch wrapped
sums). One `if`, same error style. ELF gets SIGBUS-proofing for free from this
check because the mapping is then provably inside the file.

### C4. MED [code] — PE stub: one thread per frame, uncapped

`zpe_stub.c:201` — `CreateThread` per frame. With `--pe-chunk` small (say
0.05 MiB on a 17 MB exe → ~340 frames) that is 340 simultaneous threads.
The `CreateThread == NULL → run inline` fallback keeps it *correct*, and the
frame-count cap (4096) keeps it bounded, but it's inefficient and inconsistent
with the ELF stub's `min(ncpu, nframes)` work-queue (which the docs hold up as
the v2 design lesson — §7.2 note 1).

**Fix:** cap at `min(ncpu, nframes)` workers with the same atomic-claim loop
as `zpack_stub.c` (`InterlockedIncrement`), or simpler: cap threads at ncpu and
give each a slice of the frame array. Removes the pathological case and the
wine-scaling caveat documented in §10.2.

### C5. MED — selftest covers only frame-data corruption

`selftest.sh:36-46` flips a byte at `payload_off + 16` (inside frame 0). It
never corrupts (a) a table `comp_len` (would have caught C3), (b) a table
`uncomp_len`, (c) `nframes`/`payload_off` fields. C1's PE equivalent is
likewise untested.

**Fix:** extend `corrupt_payload_byte` with a `--table N` mode that rewrites a
table entry to an absurd value; assert clean 127 (not signal death) for all of
{frame data, comp_len, uncomp_len, payload_off} on both ELF and (wine) PE.
Cheap insurance for exactly the class of bug found here.

### C6. MED — supply chain: zstd tarball fetched without checksum pinning

`build-stubs.sh:21`, `selftest.sh:52`, `.github/workflows/workflow.yaml:402`
and the README all do `curl -sL …/zstd-1.5.7.tar.gz | tar xz`. Version-pinned
but not hash-pinned; the tarball is compiled into shipped release artifacts.

**Fix:** pin SHA-256 (`sha256sum -c` after download) in the three scripts +
CI; keep the hash next to the version. Two-line change per site.

### C7. LOW [code] — PE stub: `self_subsystem()` reads OOB on crafted `e_lfanew`

`zpe_stub.c:277`: `memcmp(hdr + *(DWORD*)(hdr + 0x3C), "PE\0\0", 4)` uses the
file-supplied `e_lfanew` before any bound check (the follow-up `opt + 70 >`
check at line 279 is fine). With a corrupted stub header this reads up to 4 GB
past a 1 KB static buffer. Realistic stubs have small `e_lfanew`, so this is
hardening, not a live bug.

**Fix:** `DWORD e = *(DWORD*)(hdr + 0x3C); if (e < 0x40 || e + 24 + 70 > sizeof hdr) return 0;`

### C8. LOW [code] — PE stub: `argv[0]` claim is wrong (comment vs behavior)

`zpe_stub.c:285` says "argv[0] stays the packed exe path", but `launch_child`
builds the child command line as `"<cache-exe>" <args…>` (`zpe_stub.c:300-302`)
with `lpApplicationName = exe` — the child's `argv[0]` **is the cache path**,
not the packed exe the user ran. Harmless for byteshaver (argparse ignores
argv[0]) and for `GetModuleFileName` (same either way), but any future
argv[0]-sensitive logic would silently misbehave.

**Fix (pick one):** preserve the original `argv[0]` token verbatim as the first
command-line token while keeping `lpApplicationName = exe` (loader uses
lpApplicationName; argv[0] is purely conventional) — or fix the comment.

### C9. LOW — zpack.py edge cases crash with tracebacks instead of clean errors

Verified / by inspection:

| input | behavior |
|---|---|
| empty input file (ELF) | `ValueError: range() arg 3 must not be zero` (zpack.py:96) |
| PE with `SizeOfHeaders == 0` or a zero-length tiled gap | same crash (zpack.py:90) |
| `--pe-chunk 0` | `ZeroDivisionError` (zpack.py:88) |
| `--level -3` | becomes zstd flag `--3` → CalledProcessError traceback |
| PE section with `ptrraw+rawsz > file size` | silently truncates frames → corrupt output, no error |

CI only ever feeds well-formed linkered binaries, so these are robustness
polish: validate `--level ∈ [1,22]`, `--pe-chunk > 0`, `--page % 4096 == 0`
(`--page 2048` currently *appears* to work when the pad happens to land
4K-aligned — verified lucky pass here — then fails at runtime on the user's
machine), guard `total == 0`, and assert every PE raw range lies within the
file.

### C10. LOW — ELF stub hygiene nits

- 9 of 11 `werr(msg, n)` calls pass a length 1 byte longer than the literal
  (e.g. `werr("fexecve\n", 10)` — actual 9). Formally an OOB read of rodata;
  benign in practice. Replace with `#define WERR(s) werr(s, sizeof(s)-1)`.
- `g_rc` is a plain `volatile int` written/read across threads without atomics
  (`zpack_stub.c:63,84,85`) — a data race in the C model (works on every real
  platform since it's a sticky aligned flag). Use `__atomic_load_n/store_n`.
- `die("")` after `werr(...)` produces double messages (`"read frame table\n"`
  then `"zpk: \n"`) — fold the message into one call.
- `sysconf(_SC_PAGESIZE)` per frame in `do_frame` — hoist to main.
- Duplicate `#include <string.h>` (line 96); unused `argc` in `main`.

### C11. LOW [code] — misc PE notes

- `launch_child` truncates >32 K command lines silently (`cmd[32768]`,
  matches the CreateProcess limit, but no error on truncation).
- GUI payload + `ZPE_NO_CACHE=1` leaks a ~16 MB `zpe-<pid>.exe` per launch
  (cannot delete a running image; acknowledged in a comment) — fine for a debug
  knob, worth one doc line.
- Cache-hit path checks existence only (`GetFileAttributesA`,
  `zpe_stub.c:381`); a size check (`GetFileAttributesEx`) would catch torn
  entries for free.
- The cache directory is user-writable; a local attacker who can compute the
  FNV key can pre-plant `<key>\<name>.exe` and the stub will execute it
  without verification. Same threat class as any per-user app cache
  (equivalent access would allow planting a fake packed exe / DLLs anyway),
  but worth documenting next to the existing AV caveats. Verifying the cached
  exe against a hash would cost the warm-start advantage; documenting is the
  right trade.
- Overlapping PE raw sections (tiny-PE trick, never emitted by real linkers)
  produce overlapping frames → benign data race (identical bytes) in the
  worker threads. Rejecting `off < cur` in the tiler closes it.

---

## 2. Performance / efficiency findings

### P1. Frame compression is serial — measured 3.6× wall-clock win available

`zpack.py:100-104` compresses frames one `zstd` subprocess at a time. Measured
here (22 MB glibc binary, `-19`, 6 frames, 8 cores):

| | wall | user CPU |
|---|---:|---:|
| current (serial) | 4.98 s | ~5.8 s |
| 6 parallel `zstd -19 -T1` | **1.37 s** | 7.0 s |

**Fix:** `concurrent.futures.ThreadPoolExecutor` over the per-frame
`subprocess.run` calls (the GIL is released while waiting on subprocesses —
threads suffice, no pickling). ~10 lines; keeps frames independent (required)
and output byte-identical. On 2-core CI runners expect ~2-2.5×.

### P2. Cold-start: `posix_fadvise(WILLNEED)` on the payload — already on the
project's own roadmap (§8.1), not yet implemented. One call after `open()`,
before `mmap`, in `zpack_stub.c`. Helps first-run-after-download only; zero
warm cost.

### P3. PE stub thread cap (C4) is also a perf item — 340 threads → 8 threads
with the same or better wall time, and consistency with the ELF stub's
measured queue design.

### P4. Micro / not worth it (recorded so they aren't re-litigated):
- `zpack.py` slice copies (~2× file RSS in the packer) — memoryview saves
  nothing measurable at 16-22 MB; skip.
- `pread_range` reopens the self file 3× in zpe — µs; skip.
- MADV_SEQUENTIAL on input — superseded by the existing per-frame DONTNEED.
- Parallel decompression beyond ~6 workers — measured bandwidth-bound (§7.3);
  correctly left alone.
- Boundary-page sharing between adjacent frames can cost one extra minor
  fault per boundary when a worker's `madvise` drops a page a neighbor is
  still reading (correct — clean private pages repopulate — just a handful of
  faults). Ignore.

### P5. CI efficiency
- The alpine docker stage rebuilds libzstd twice (host + mingw) per run;
  caching `packer-out/` stubs keyed on the zstd tarball hash + stub sources
  would cut ~1-2 min per release run. Only worth it if release runs are
  frequent.
- PE smoke under wine is absent from CI (selftest supports it if wine is
  installed). Adding one `apt install wine64` step makes the PE path
  regression-tested per release — recommended before calling packed Windows
  artifacts production-ready (the docs themselves flag "real-Windows smoke
  test in CI remains").

---

## 3. Improvement plan (phased, smallest-risk ordering)

**Phase 0 — ship blockers (≤ half a day total)**
1. C1: bounds-check `comp` lengths in `zpe_stub.c` (+C5 test legs).
2. C2: per-PID `.tmp` name in `write_cache_exe`.
3. C3: `payload_off + payload_len ≤ fsz - 24 - tbl_sz` check in `zpack_stub.c`
   (+C5 test legs).
4. C6: SHA-256-pin the zstd tarball in build-stubs.sh / selftest.sh / CI.
5. Re-run `selftest.sh` (ELF legs) + wine legs where available; bump the
   docs' robustness claims once the table-corruption cases pass.

**Phase 1 — should have (≈ a day)**
6. P1: parallel frame compression in zpack.py.
7. C4/P3: worker-cap + queue in the PE stub.
8. C7: `e_lfanew` bounds in `self_subsystem`.
9. C9: zpack.py argument/file validation (incl. `--page % 4096`).
10. C8: argv[0] preservation (or comment fix) in `launch_child`.

**Phase 2 — polish (opportunistic)**
11. P2 `posix_fadvise(POSIX_FADV_WILLNEED)`; C10 hygiene (WERR macro, atomic
    g_rc, folded error messages); cache-entry size check (C11); overlapping-
    section rejection (C11); wine PE smoke in CI (P5); P5 stub caching.

**Phase 3 — strategic (optional)**: see §4.

Each phase leaves the container format (`ZPK2zstd`/`ZPK3pe64`) untouched —
old packers produce files new stubs accept and vice versa; no format-version
bump is required for anything above.

---

## 4. Would Rust improve toolchain efficiency/coherency?

Split by component — the answer differs sharply:

### 4a. The packer (`zpack.py` → Rust workspace member): worthwhile, zero performance cost

| axis | assessment |
|---|---|
| compression speed / ratio | **identical** — the `zstd` crate compiles the same vendored libzstd C sources via `libzstd-sys`/cc; the artifact bytes are the same for the same level/params (pin crate ↔ vendored version, 1.5.7, to keep them in lockstep) |
| packing wall time | **better** — P1's parallel compression becomes `std::thread` over an in-process `zstd::bulk::compress`, no subprocess spawn per frame |
| safety | the PE-header parsing in `zpack.py` (`parse_pe64`, struct offsets into a byte buffer) is exactly the code class Rust checks at compile time; C1/C9-class bugs (bounds, truncation) stop being review-dependent |
| coherency | the release path currently needs: cargo, python3, zstd CLI, gcc/musl, mingw, bash. A `tools/packer/zpack` cargo bin (workspace member, `exclude`d like jpegxl-src is not needed — plain member) removes python3 + the zstd CLI runtime dep; the container-format constants live in typed Rust next to unit tests instead of in a Python docstring |
| CI | one `cargo build --release -p zpack` step replaces the python invocation; static Linux binary, no interpreter variance |
| cost | ~250-350 LOC rewrite, one new dependency tree (`zstd` + `libc`), re-run of selftest (which drives `zpack.py` by path — swap the invocation) |

**Recommendation:** do it as Phase 3, bundled with a real need to touch the
packer (e.g. when P1 is implemented anyway, or when aarch64 artifacts force
`--page` logic per target). Doing it *now* buys coherency, not correctness —
the Phase 0/1 fixes should not wait for it. If it is done, keep the Python
script one release cycle as a cross-check (pack with both, assert identical
output hashes in CI).

### 4b. The stubs (C → Rust): **no — keep C**

- **Size:** the stub's 92 KB is per-artifact overhead that the current design
  worked hard to reach (194→92 KB documented). A `std` Rust static-musl
  binary starts at ~300-600 KB even after `panic=abort`/LTO tuning — 3-6×
  worse on the one axis every packed artifact pays for. A `no_std` stub can
  approach C size only by re-implementing what the C stub already is (raw
  syscalls, no alloc), while *adding* `unsafe` libc declarations for
  `memfd_create`/`fexecve`/`madvise` that std doesn't provide. You would
  rewrite the same 170 lines with more ceremony and a bigger audit surface.
- **Runtime:** startup is dominated by zstd decode + page faults (measured:
  §4.1 of the analysis doc). Same syscalls, same library → no headroom for a
  language win. The measured numbers would not improve; the risk of
  regressing the 8.9 ms/17 MB results during a rewrite is real.
- **Audibility:** "tiny C stub + pinned zstd" is a 30-minute security review;
  a Rust std pull-in is not, and packer stubs are exactly the kind of code
  AV vendors and enterprise security teams eyeball.
- The C stubs are format-stable, tested, and measured. Leave them.

### 4c. Net answer

Yes for the packer tool (efficiency: neutral-to-better; coherency: clear win;
risk: low, bounded, verifiable by byte-identical output). No for the stubs
(size regression on every artifact, zero runtime upside, larger audit
surface). This split also matches where the bugs actually were: all Phase-0
correctness issues live in hand-rolled parsing/dispatch code — which is either
deleted (packer → Rust) or is 6-line fixes (stubs → stay C).

---

## 5. Verification log (this audit)

Environment: repo @ facc9e0, gcc 15/glibc host stub (functional stand-in for
the musl build), vendored zstd-1.5.7.

- Built `zpack_stub.c` statically; round-trip PASS: frames 1/2/4/13, arg
  passthrough incl. spaces from foreign cwd, exit codes 42/7, `--page 65536`
  on 4 K kernel, `ZPK_THREADS=1`.
- Corruption matrix: frame data → 127 ✓; `uncomp_len` → 127 ✓; `payload_off`
  → 127 ✓; `comp_len` → SIGBUS ✗ (C3).
- Truncated packed file → SIGSEGV at execve — **not a zpack bug**: a truncated
  plain static ELF fails identically (verified with the unpacked payload).
- zpack.py: empty input → ValueError traceback (C9).
- Packing time: serial 4.98 s vs 6-way parallel 1.37 s wall / 7.0 s user on a
  22 MB payload, `-19` (P1).
- werr length audit: 9/11 literals overread by 1 byte (C10).
- PE stub: not executable here (no mingw/wine) — C1/C2/C4/C7/C8 are
  code-review findings; each cites line numbers for re-verification on a
  windows-capable box via `tools/packer/selftest.sh`.

---

## 6. Remediation log (2026-10-05)

All Phase-0 and Phase-1 items plus selected Phase-2 items landed; the PE-side
code findings were then verified for real (llvm-mingw 20260922 + wine 10 on
the audit machine).

| fix | commit | verification |
|---|---|---|
| C10 ELF hygiene (die() paths, atomic g_rc, pagesize) | `chore(packer)` | glibc stub byte-size unchanged; behavior identical |
| C3 ELF extent validation (SIGBUS → 127) | `fix(packer)` | corrupt comp/uncomp/off/data all → clean 127 (was SIGBUS for comp) |
| C1 PE comp-len bounds | `fix(packer)` | corrupt matrix under wine all 127 (was a wild read for comp) |
| C2 per-PID cache tmp + tmp sweep | `fix(packer)` | double-launch staging no longer shares a file |
| C11 cache-entry size check (torn-entry recovery) | `fix(packer)` | truncated cache entry detected, re-extracted, self-healed |
| C7 e_lfanew bounds | `fix(packer)` | bounded before deref |
| C8 argv[0] preservation | `fix(packer)` | wine: child argv[0] = invoked packed path (was cache path) |
| C4 PE worker cap (queue) | `perf(packer)` | round-trip/serial/17-frame/corrupt all pass under wine |
| C9 zpack.py validation | `fix(packer)` | 7 malformed-input cases → clean `zpack:` errors (was tracebacks/silent corruption) |
| P1 parallel compression | `perf(packer)` | byte-identical output (sha256), 4.9 s → 1.4 s wall (6 frames, -19) |
| P2 fadvise(WILLNEED) | `perf(packer)` | cold median 17.1 → 16.1 ms; warm unchanged |
| C5 selftest corruption legs | `test(packer)` | full run: 14 passed, 0 failed (ELF + wine PE) |
| C6 zstd sha256 pinning | `fix(packer)` | good hash passes, one-bit-wrong hash aborts |

Measured overhead of the whole remediation (the question this audit had to
answer before production):

| metric | before | after | delta |
|---|---:|---:|---|
| ELF stub size (glibc test build) | 916,296 B | 916,296 B | **0 B** |
| packed startup, warm (hyperfine, interleaved, 200 runs) | 12.1 ms ± 0.8 | 12.1 ms ± 0.8 | **1.00× ± 0.09** |
| packed startup, cold (FADV_DONTNEED eviction, 15 runs) | 17.1 ms | 16.1 ms | −1 ms (improved) |
| PE stub size (llvm-mingw static) | 78,848 B | 79,360 B | +512 B (+0.65 % stub ≈ 0.01 % artifact) |
| PE warm path | cache hit | cache hit + one GetFileAttributesEx | sub-µs |
| pack wall time (22 MB, -19, 6 frames) | 4.9 s | 1.4 s | **3.5× faster** |

The validation loops are O(nframes) at startup (≤ 4096 iterations of a
compare each, well under a microsecond) — invisible next to the ~10 ms
decompression they guard. Remaining open items from §3: CI wine smoke for
packed PE artifacts (Phase 2), the Rust packer rewrite assessment (Phase 3,
unchanged recommendation).
