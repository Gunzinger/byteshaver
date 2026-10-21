# Adversarial Codebase Audit — 2026-10-14

**Scope:** entire repository at `d4feb49` (`main`) — core engine (`src/`), converters (incl. in-tree libjxl FFI), input/metadata layers, GUI (`gui/`), custom binary packer (`tools/packer/`), build system (`build.rs`, `.cargo/`), CI/CD and Docker.

**Method:** five parallel adversarial review passes (core pipeline, converters/FFI, decoders/metadata, GUI, build/packaging), each reading the full files in scope and verifying claims against dependency sources (vendored libjxl 0.12, ravif 0.13, webp 0.3.1, image 0.25.10, kamadak-exif 0.6) and, where possible, against the built binary. Top findings were independently re-verified; four were reproduced empirically (marked **[repro]**). Line numbers refer to the audited commit.

---

## Executive summary

The codebase is in notably good shape for its size: the hand-written parsers are bounds-checked with zero `unsafe`, the libjxl FFI layer has correct struct layouts, RAII scoping and defensive buffer math, per-file error isolation works, clippy is essentially clean, and virtually all GUI `unwrap()`s live in `#[cfg(test)]` modules. The genuine problems cluster in four places:

1. **Whole-process death instead of per-file errors.** The release profile (`panic = "abort"`, `overflow-checks = false`) plus unvalidated CLI numbers, `println!` on EPIPE, disabled decoder limits, symlink-cycling recursion, and pre-cap animation buffering means single bad inputs or mundane usage (`byteshaver ... | head`, `avif --quality 0`) abort the entire batch with all progress lost.
2. **A non-atomic, non-contained write path.** Output writes have no temp+rename, in-place same-extension conversion is a silent no-op by default and data destruction with `--overwrite-*`, and an absolute-path `join` can scatter outputs outside `-o`.
3. **Fail-open metadata policy.** The EXIF privacy filter keeps unparseable payloads verbatim — an attacker (or a mere maker-note-heavy JPEG) bypasses `--exif-except gps` silently; the JXL input path can double-rotate images.
4. **Weak packaging/supply chain.** The PE packer stub has a wrapping-arithmetic bounds bypass and a size-only cache check; the compiler toolchain and codec sources are downloaded without checksums; actions are tag-pinned; `cargo publish` runs before artifacts are validated; the published Docker image floats on `alpine:latest` as root.

---

## CRITICAL

### CR-1 · `avif --quality 0/NaN/150`, `--speed 0`, `--alpha-quality 0` abort the whole batch **[repro]**
`src/cli.rs:159-167,182-185` (plain `Option<f32>/<u8>`, no range parser) → `src/converter/avif.rs:203-208` → ravif `assert!(quality >= 1 && quality <= 100)` and `speed` 1..=10 (`ravif-0.13.0/src/av1encoder.rs:111,140,153`).
The CLI doc *says* "0 - 100", so a doc-following user kills every in-flight conversion: reproduced as `thread panicked at src/converter/avif.rs:204 … Error: job worker thread panicked`; in release (`panic = "abort"`) it is a hard SIGABRT.
**Fix:** `value_parser!(f32).range(1.0..=100.0)` for quality/alpha-quality, `range(1..=10)` for speed, or validate in `AvifOptions` and return an error.

### CR-2 · Directory symlink cycle → stack overflow abort
`src/pipeline.rs:722-734` (esp. `path.is_dir()` at 728, which follows symlinks) and 742. A `ln -s . loop` inside an input directory makes `collect_directory` recurse into itself forever; stack overflow aborts the process regardless of panic settings — no per-file error, no Ctrl+C window, hours-long batch gone. (Symlinked dirs are common in synced/album folders.)
**Fix:** use `entry.file_type()?.is_dir()` (non-following) from `DirEntry`, track canonicalized paths in a `HashSet`, and add a depth limit.

### CR-3 · Decoder limits explicitly disabled → decompression bomb aborts the process
`src/input/mod.rs:361` (`reader.no_limits()`) and `:417` (`Limits::no_limits()`). A crafted PNG/WebP/TIFF declaring 100000×100000 px (~40 GB RGBA) is not rejected; the `Vec` allocation failure aborts (uncatchable), and parallel rayon workers multiply the effect. The pipeline's "huge image" notice (`src/pipeline.rs:635-655`) runs too late.
**Fix:** set real `Limits` (configurable `max_alloc`), treat violations as per-file errors.

### CR-4 · Animation frames fully decoded before the memory cap is checked (+ overflow in the projection math)
Frames are materialized as full-canvas RGBA buffers at `src/input/mod.rs:226-230` (GIF), `:293-297` (WebP), `:316-322` (APNG), `src/input/jxl.rs:214-220` (JXL); the `--max-animation-memory` cap is only evaluated afterwards at `src/pipeline.rs:607-620`. A 500-frame 4000×4000 GIF (~32 GB) OOM-aborts before the guard runs. Additionally `u64::from(w) * u64::from(h) * 4` (`src/pipeline.rs:442`) wraps in release (`overflow-checks = false`): a crafted 2³¹×2³¹ header projects to **0 bytes** and passes the cap check.
**Fix:** probe dimensions/frame count from the decoder header before decoding; use `checked_mul`; enforce the budget incrementally while frames accumulate.

### CR-5 · PE packer stub: wrapping-arithmetic bounds bypass → out-of-bounds read fed to zstd
`tools/packer/zpe_stub.c:405`: `payload_off + 24 + nframes * 24 > self_size` is evaluated in wrapping `uint64_t`; a corrupted/truncated file with `payload_off = 2⁶⁴ − 24 − 24·n` passes, `frames_end - src` (line 222) wraps as well, and `jobs[i].src = g_self + src` (line 225) points below the heap buffer — `ZSTD_decompress` then reads out of bounds. The ELF sibling got this right (`zpack_stub.c:128` orders the comparison before any subtraction); the PE stub's own comment (214-216) claims this attack is prevented — it isn't.
**Fix:** reject `payload_off >= frames_end` with a non-wrapping comparison before any subtraction.

### CR-6 · Non-atomic output writes + in-place same-extension semantics: destroyed originals and silent no-ops **[repro]**
`src/pipeline.rs:690` writes with plain `fs::write` (no temp+rename). Combined with:
- **In-place conversion** (`output_path == input_path` when no `-o` and target ext == source ext, `src/pipeline.rs:415-416`): default policy makes the flagship re-optimization use case a *silent no-op* — reproduced: `byteshaver '*.png' png` → `Successful: 0, Skipped: 3`, nothing converted, no warning.
- With `--overwrite-existing`/`--overwrite-if-smaller` it becomes in-place mutation backed only by the non-atomic write: disk-full, crash or `kill -9` mid-write **destroys the original photo irrecoverably**.
- TOCTOU: exists-check (571-574) → metadata (668) → write (690) is racy across processes; `claimed_outputs` only guards one process.
**Fix:** temp-file + `fs::rename`; detect `output == input` and hard-error (or warn loudly and force the atomic path); exclusive-create when policy is `KeepExisting`.

---

## HIGH

### H-1 · `println!` on closed stdout panics → poisoned-mutex cascade / release abort **[repro]**
`src/job/reporter.rs:248-255, 333-407` print while holding the state mutex (locked at `:419`); poisoning then trips `.expect("mutex poisoned")` at `:419`, `:483` and `src/pipeline.rs:294, 911, 935`. Reproduced: `byteshaver '*.png' webp -o out | head -c 5` → `panicked … failed printing to stdout: Broken pipe` → `Error: job worker thread panicked`. In release this aborts mid-batch.
**Fix:** use `writeln!(io::stdout(), ...)` and ignore/handle `Err` (disable stdout rendering on EPIPE); never hold the state mutex across I/O.

### H-2 · EXIF filter policies fail OPEN — privacy bypass
`src/metadata/exif.rs:141-152`: on parse or re-serialization failure the filter *keeps the payload verbatim*. With `--exif-except gps`, any EXIF `kamadak-exif 0.6` can't round-trip (truncated files, heavy maker notes — it is stricter than exiftool) bypasses filtering entirely; only a `println!` warning hints at it. The pipeline re-embeds the unfiltered bytes at `src/pipeline.rs:497`.
**Fix:** fail closed — strip (loud per-file warning) or fail the conversion; never embed unfiltered bytes under a filtering policy.

### H-3 · JXL `--intensity-target` silently ignored
`src/converter/jxl/mod.rs:463` hard-codes `intensity_target: 255.0`; the option field (114-115) is only read by `Display`. Verified: `--intensity-target 1000` vs `255` produce byte-identical output. HDR users get silently SDR-clamped pixels.
**Fix:** `options.intensity_target.unwrap_or(255.0)` and validate > 0.

### H-4 · EXIF policy resolved twice — library/GUI path silently strips metadata
`src/converter/jxl/mod.rs:363-366` and `src/converter/oxipng.rs:442-447` re-resolve `resolve(&self.options.exif_policy, …)` after the pipeline already resolved the global policy into `input.metadata.exif` (`src/pipeline.rs:480,497`). `JxlOptions`/`OxipngOptions` default to `Strip` (`src/config.rs:330,354`) and are only patched in the CLI binary (`src/main.rs:73-80`) — any `JobHandle::start` caller (GUI, library users) with `exif = Keep` gets JXL/oxipng outputs with no metadata, no warning, and `metadata_dropped` uncounted.
**Fix:** delete the encoder-local re-resolve; use the already-resolved `input.metadata.exif` like every other encoder; remove the duplicated `exif_policy` field.

### H-5 · JXL animation encoding: two extra full-frame copies per frame
`src/converter/jxl/mod.rs:250` (`frame.buffer.clone()` into `DynamicImage`, then `to_rgba8()` clones again) plus `EncodeRequest`'s `Vec<PreparedFrame>` (214-225) duplicating every frame buffer. A 300-frame 1080p animation pays ~5 GB of transient memcpy and ~2× peak memory — and it under-enforces `--max-animation-memory` by ~2× because the cap only counts source buffers.
**Fix:** normalize from `&RgbaImage` without the `DynamicImage` wrapper and clones, or stream frames into libjxl.

### H-6 · GUI: stale `discovered` rows leak into the next run — double processing
`gui/src/app.rs:906` builds the `JobSpec` from `Queue::selection()` (`gui/src/queue.rs:463-465`, which includes `discovered: true` rows) *before* `begin_run` (app.rs:920) clears them (`queue.rs:456-458`). The core's `discover_files` (`src/pipeline.rs:739-750`) does not dedup, so Convert-after-drop re-processes the previous directory expansion: with default policy everything re-reports as `SkippedExisting` (2× totals); with "overwrite" the whole tree is genuinely re-encoded.
**Fix:** filter `!item.discovered` in `selection()`, or clear discovered rows before building the spec.

### H-7 · GUI: full recursive directory walk + O(n²) dedup on the UI thread
`gui/src/queue.rs:138-173, 259-292, 430-441`: a dropped folder triggers recursive `read_dir`+`metadata` per entry, per-path double canonicalization and linear queue scans — inside `logic()`. Large or network-mounted libraries freeze the window for seconds to minutes.
**Fix:** move scan/stat to a worker thread (the thumb worker exists); dedup via `HashSet` of canonical keys.

### H-8 · Supply chain: compiler toolchain and codec sources downloaded without checksums
`.github/workflows/workflow.yaml:123-124`, `tools/packer/setup-wsl.sh:21-24`, `tools/libheif-static/build-decode-only.sh:24-36` (and the `-windows.sh` twin): llvm-mingw (which compiles *every shipped Windows binary*) and the libde265/dav1d tarballs are `curl | tar`'d with version strings only — no SHA-256; the dav1d URL is a gitlab archive endpoint whose bytes aren't stable across regenerations. A compromised upstream asset silently backdoors release binaries. (zstd, by contrast, is checksum-pinned at every site — the pattern is known in-repo.)
**Fix:** SHA-256 verification for llvm-mingw and both codecs; serve dav1d from release artifacts.

---

## MEDIUM

### Core engine (`src/`)
- **M-1 · Output claimed before input validation** — `src/pipeline.rs:551-570`: a corrupt `photo.png` claims `photo.jpeg`, then fails decode; the valid `photo.jpg` mapping to the same output is reported `SkippedCollision`. Neither converts; the claim is never released on error.
- **M-2 · Absolute-path `join` escape** — `src/pipeline.rs:420-427`: when `strip_prefix` fails, `rel_path` is the full absolute input path and `Path::join` with an absolute component *replaces* the base: outputs silently leave `-o` (and, with overwrite flags, can touch files the user never targeted). Fall back to `output.join(file_name())` or error.
- **M-3 · Unvalidated numeric flags beyond avif** — webp `quality` (`src/cli.rs:150-152`, NaN/1e9 pass `WebPValidateConfig`'s `<0||>100` check), webpanim `method`, `kmin/kmax: Option<i32>` (`:328-333`, negatives accepted), gif `--speed` any i32 (`:372`), `--max-animation-memory 0` errors every animation. JXL shows the right pattern (`src/config.rs:299-315`); apply it everywhere.
- **M-4 · Reporter hot-path churn** — 3 events/file each taking the single `StdoutState` mutex (`src/job/reporter.rs:419`), two of them no-ops (`:450`); ~4 `PathBuf` + 2 `Outcome` clones per file (`src/pipeline.rs:881-930`); progress string reformatted per file (`:438`). Serializes many-core runs.
- **M-5 · `panic = "abort"` makes panic recovery dead code** — `src/job/mod.rs:267-278`'s join-time recovery only works with unwinding; in the shipped profile any worker panic aborts the whole process including the in-process GUI. Every `.expect("mutex poisoned")` (`src/pipeline.rs:294,911,935`; `reporter.rs:117,419,483`) is a kill-switch. Prefer `unwrap_or_else(|p| p.into_inner())` where guards can't break invariants; reconsider the profile.
- **M-6 · 4× materialization of every discovered path; unbounded results** — `src/pipeline.rs:796-799, 855-869` (redundant producer thread + unbounded channel; `into_par_iter` is equivalent), `878-917` (full `FileResult` per item retained until the end). Multi-million-file globs OOM before the first decode.
- **M-7 · Silent discovery error swallowing** — `src/pipeline.rs:797` (`filter_map(|e| e.ok())`), `723-725`: permission-denied dirs and glob errors vanish without notice.
- **M-8 · `clean` subcommand** — `src/utils.rs:44-53`: irreversible glob deletion, no confirmation/dry-run, partial-run state unreported, `u64 as usize` truncation.
- **M-9 · Ctrl+C can't interrupt a running encode; no timeouts anywhere** — stop flag checked once per item (`src/pipeline.rs:883`); a pathological file runs for hours; second Ctrl+C only prints more exclamation marks (`src/main.rs:20-31`). Escalate to hard exit on Nth signal; optional per-file timeout (oxipng already has `timeout_secs`).
- **M-10 · GUI crate defeats the feature escape hatch** — `gui/Cargo.toml` declares `byteshaver = { path = ".." }` without `default-features = false`, so any workspace build (including `cargo clippy --workspace`) compiles the vendored libheif even when dec-heif was opt-ed out elsewhere.

### Converters (`src/converter/`)
- **M-11 · Silent 8-bit quantization of 16-bit / float input** in every target except (partially) JXL: `avif.rs:212-230` (even with `--bit-depth ten`, feeding fake 10-bit), `png.rs:160-174`, `mozjpeg.rs:99`, `webp_image.rs:73-89`, `apng.rs:152-153`, `jxl/mod.rs:286-295` (f32 crushed to 8-bit despite `JXL_TYPE_FLOAT` being declared). No warning, no opt-out; PNG output silently loses depth the format supports.
- **M-12 · ravif thread oversubscription** — `avif.rs:206` passes `threads: None`, which makes rav1e size its *own* pool at `rayon::current_num_threads()` per file (the code comment claiming "shared global rayon pool" is wrong); `ThreadBudget::per_files()` is never called anywhere. N concurrent files × C rav1e threads on C cores (measured 11 threads for one 1200² encode on 8 cores).
- **M-13 · libjxl encoder fully single-threaded** — no `JxlEncoderSetParallelRunner` is ever registered (`ffi.rs` doesn't even declare it); one file = one core; effort 7-10 encodes ~N× slower than cjxl on N cores.
- **M-14 · `webp` target hard-fails on 16-bit/float input** with cryptic `"Unimplemented"` (`webp.rs:106-123`), while all other targets downconvert. Downconvert + notice instead.
- **M-15 · `JxlFrameHeader` dead construction** — `jxl/mod.rs:594-615`: the hand-filled `layer_info`/`timecode`/`is_last` are zeroed by `JxlEncoderInitFrameHeader` (verified in vendored encode.cc:1277-1300); output is accidentally correct; a future "cleanup" can break animations.
- **M-16 · Avoidable full-image copies in every hot path** — at least one full memcpy per still (avif/png/mozjpeg/webp/apng/oxipng/gif/traits.rs:122) plus `.to_vec()` of encoded webp buffers (`webp.rs:129`, `webp_anim.rs:203`). Borrow `DynamicImage::ImageRgba8(p).as_raw()` where crates accept slices.
- **M-17 · oxipng passthrough re-reads the input from disk** (`oxipng.rs:437`) although the pipeline already read it for format detection.

### Input / metadata (`src/input/`, `src/metadata/`)
- **M-18 · JXL silent orientation-strip failure → double-rotated output** — `src/input/jxl.rs:68` sets `exif_applied_orientation = true` unconditionally; if the Exif box fails to parse/re-serialize (272-274, 293-295) the Orientation tag survives while jxl-oxide already baked rotation into pixels; the pipeline (488) then skips its own correction. The HEIF twin handles this (`heif.rs:128-132` warns + resets the flag); JXL doesn't even warn.
- **M-19 · Whole-file reads for metadata** — `src/metadata/mod.rs:119` and `exif.rs:42` `fs::read` the entire file (10 GB TIFF → 10 GB heap) to scan for EXIF, after decode already parsed it; `read_summary` is called per file by the GUI table. Also `fallback_retry_read_image` performs a second full JPEG decode (`input/mod.rs:396-413`) and EXIF is parsed twice per file (`pipeline.rs:478,483`).
- **M-20 · Missing `catch_unwind` on jxl-oxide / image-crate decode paths** — the HEIF module wraps decoding in `catch_unwind` (`heif.rs:83`) precisely because decoders panic on hostile input; the jxl and generic paths (invoked from rayon at `pipeline.rs:881`) have no such guard — one decoder panic kills the batch instead of one `Outcome::Error`.
- **M-21 · JXL sample-buffer allocation has no dimension guard** — `src/input/jxl.rs:127-129` (`w*h*channels` then `vec![0u16; …]`): a small crafted file with ~2³⁰ declared dimensions drives a multi-exabyte allocation → uncatchable capacity-overflow abort; on 32-bit the multiply can wrap. Cap dimensions, use `checked_mul`.
- **M-22 · TIFF EXIF fallback can embed the whole input file as the "EXIF payload"** (`src/metadata/mod.rs:124-127`): with `--exif-keep`, PNG output gains an `eXIf` chunk ≈ input size (JPEG saved only by the 64 KB APP1 guard, `sink.rs:21`).

### GUI (`gui/`)
- **M-23 · Worker panic leaves the app permanently "running"** — `gui/src/app.rs:986`: `drain_events()` returns on `Disconnected` without `finish_job()`; `running` stays `Some(...)`, Convert disabled, 100 ms repaint loop forever, Cancel flag unread. Synthesize an error report on disconnect.
- **M-24 · Unreaped children → zombie accumulation on Unix** — `gui/src/platform.rs:253-259, 57-96`: every "open file"/reveal click leaks a zombie for the app's lifetime (gdbus watcher gives up after 1 s without `wait()`).
- **M-25 · Blocking `rfd` file dialogs on the event loop** — `gui/src/panels/drop_zone.rs:149-161`: portal round-trips freeze painting (and job progress UI) for seconds. Use `AsyncFileDialog`.
- **M-26 · Preset rename deletes the old file before writing the new one** — `gui/src/presets/mod.rs:637-651`: failure between the two steps loses the preset outright (the individual write *is* atomic).
- **M-27 · Settings save is non-atomic and per-frame during drags/resize** — `gui/src/settings.rs:459-467` plain `fs::write`; `app.rs:1296-1310` + `options_panel.rs:312-319` mark dirty at 60 Hz while interacting → JSON serialize + `fs::write` per frame, with a real torn-write window. Atomic write + debounce.
- **M-28 · Measure/Inspect requests silently dropped when one is in flight** — `gui/src/metrics/worker.rs:215-230`: shared `pending` set keyed by input path; the inspector window waits forever.
- **M-29 · O(n²) event application at run end** — `gui/src/app.rs:977-1038` + `queue.rs:488-515`: each event linearly searches items by path; discovered files stat + sniff on the spot. A 5k-file run ends with ~12.5M comparisons + 5k stats in one frame — a multi-second freeze exactly at completion. Index rows by path; make discovered rows stat-free.
- **M-30 · Full re-sort every frame with allocating comparators** — `gui/src/table.rs:275-329`, `panels/file_table.rs:350-351`: ~260k `String` allocations/frame for a 10k-row queue. Precompute sort keys; cache the permutation.
- **M-31 · Full `RunReport` clone every frame while the report window is open** (`gui/src/panels/report.rs:95`) plus a 200 ms perpetual repaint of a static window.
- **M-32 · Thumbnail/metrics queues accumulate unboundedly while paused; no stale-request invalidation** (`gui/src/thumb.rs:286-315`) — scrolling a 10k-row queue during a run enqueues 10k decodes for rows no longer visible.

### Packer / build / CI / Docker
- **M-33 · PE cache poisoning via size-only check** — `tools/packer/zpe_stub.c:456-465`: steady-state launches `%LOCALAPPDATA%\byteshaver\zpe-cache\<key>\<name>.exe` after checking only file *size*; the FNV key is derivable from the packed file; any same-size replacement yields persistent code execution. Store + verify a SHA-256 of the reconstructed payload.
- **M-34 · Unbounded `wsprintfA` into undersized stack buffers** — `tools/packer/zpe_stub.c:283-285, 493-495, 64-72`: up to ~14-byte stack smash with `ZPE_DEBUG=1` and near-`MAX_PATH` cache paths.
- **M-35 · Packer never validates frame count against the stubs' 4096 cap** — `tools/packer/zpack.py:68,119-124` vs `zpe_stub.c:405`: `--frames 10000` produces a success summary and a permanently broken executable (dies at first run).
- **M-36 · Compression delegated to ambient `zstd` on PATH; decoder pinned to 1.5.7** — `tools/packer/zpack.py:129`: no version check at pack time; a runner-image zstd bump can emit frames the embedded decoder rejects (invisible for PE, see M-37).
- **M-37 · Packed Windows artifacts ship with zero execution testing** — `workflow.yaml:423-452` runs no selftest/wine leg; only the Linux `-packed` binary is smoke-tested; a real-Windows stub regression publishes broken `-packed.exe` with green checks.
- **M-38 · No `permissions:` block on any workflow job** — `workflow.yaml` (file-wide): jobs holding release/registry credentials inherit repo-default scopes. Add top-level `permissions: contents: read` + minimal per-job grants.
- **M-39 · All third-party actions pinned by mutable tags** — `workflow.yaml:23,28,110,138,344,…`: force-pushed tags execute arbitrary code in credentialed jobs. Pin to commit SHAs.
- **M-40 · `cargo publish` runs before any artifact is built or validated** — `workflow.yaml:155-173` (with `--allow-dirty` over sed-rewritten manifests): later failures can't unpublish. Gate on `needs: [build, validate]`.
- **M-41 · Floating base images / unpinned packages; published image runs as root** — `Dockerfile:24,47` (`FROM rust:1-alpine`, tagless `FROM alpine`), `workflow.yaml:244-247,402`: non-reproducible, silently re-based, root-owned outputs into user mounts.
- **M-42 · `build.rs` rebuild triggers don't cover actual inputs** — `build.rs:50,114`: nothing watches `tools/jpegxl-src/**`, `Cargo.toml`/`Cargo.lock` (versions.rs) or patch state; incremental dev builds link stale libjxl / embed stale versions.
- **M-43 · Global `target-cpu=x86-64-v3` with no runtime guard** — `.cargo/config.toml:3`: pre-Haswell contributors get SIGILL in `cargo test`; suffixless release artifacts crash with illegal instruction instead of a readable error (README discloses it; the binary doesn't). CI deliberately works around this config — the footgun is confirmed in-repo.

---

## LOW (selected)

- **Core:** per-file `create_dir_all` syscall (`pipeline.rs:589-594`); `SeqCst` on all 14 stat atomics (`:256-280`); `ProgressStats` emitted but ignored by the default reporter; `--exif-list-tags` bypasses clap (`main.rs:57-60`); `unreachable!` reachable via public `from_args` API (`config.rs:177-179`); lossy `to_string_lossy` path round-trip for non-UTF8 output dirs (`pipeline.rs:790-792`); HEIF multi-image containers re-probed once per image (`pipeline.rs:326-340`).
- **Converters:** `ffi.rs:253` doc states the wrong version packing (decoder is right — fix the doc before someone "fixes" the code); jxl errors numeric-only (`jxl/mod.rs:724-748`); `println!`s from rayon workers bypassing the Reporter (traits.rs:117, avif.rs:179, png.rs:144, webp.rs:88, mozjpeg.rs:57) + hand-rolled version lookups duplicating `dependency_version()`; APNG frame 0 compressed twice (`apng.rs:175-186`, deliberate — deserves a comment/option); `delay.as_millis() as i64` truncation (`webp_anim.rs:196`); `u16_ne_bytes` per-sample interleave (`jxl/mod.rs:305-311`).
- **Input/metadata:** RIFF writer saturates size fields to `u32::MAX` instead of erroring (`riff.rs:107-118`); `vp8x_payload` underflow on 0×0 (`riff.rs:302-303`); extension-only routing for JXL/HEIF (a `.png`-named JXL gets the wrong decoder, `input/mod.rs:123,137`); HEIF `row_len * height` can overflow usize (`heif.rs:192-193`, harmless in release due to per-row bounds checks); O(n²) duplicate-suppression `Vec::contains` scans (exif.rs:187, heif.rs:323, jxl.rs:276).
- **GUI:** LRU `promote` O(cap) per hit (`thumb.rs:131-136`); UI-thread dimension reads and mtime stats (`app.rs:1100-1112`, `metrics/worker.rs:184`); preset menu rebuilt every frame (`options_panel.rs:750-763`); import re-parses the whole preset dir per file (`app.rs:715-738`); JSONL export blocks UI thread (`report.rs:281-298`); `xdg-open` with dash-prefixed filenames (`platform.rs:158-163`); `.expect` on reporter mutex in worker context (`gui/src/reporter.rs:35`).
- **Packer/build:** memfd fallback only handles `EINVAL`, not `vm.memfd_noexec` `EPERM` (`zpack_stub.c:41-45`); `fexecve` breaks `/proc/self/exe` self-locating payloads (`zpack_stub.c:105,184`); `.tmp` sweep case-sensitive (`zpe_stub.c:311`); zpack.py PE-subsystem patch silently skipped on malformed stub + unvalidated stub header (`zpack.py:144-148`); packed exes lose version resources/icon/signatures; `apply.sh` idempotency by grep marker + fuzzy patch (`tools/patches/apply.sh:18-22`); libheif-static scripts short-circuit on artifact existence, ignoring version bumps (`build-decode-only.sh:39,57`); cache `restore-keys` prefix fallback trusts stale codec archives (`workflow.yaml:214-233`); repo-variable interpolated into shell/compiler flags (`workflow.yaml:56-61`); Docker Hub password (not PAT) exposed to two third-party actions (`workflow.yaml:569-604`); `.dockerignore` misses `packer-out/`, `selftest-out/`, `zstd-1.5.7/`, local env files; clippy: `build.rs:59` collapsible `if`, `load_heif_source` dead in non-heif builds (`input/mod.rs:211`).

---

## Verified-safe (explicitly checked, no finding)

- **libjxl FFI**: all transcribed structs match vendored 0.12 headers field-for-field; input buffers are copied internally (no dangling pointers); RAII `Encoder` guard releases on every error path; `collect_output`'s spare-capacity + `set_len` pattern is correct; buffer sizes validated before calls; one encoder per rayon task — legitimately `Send + Sync`.
- **Hand-written parsers** (`riff.rs` parse path, exif glue, orientation): bounds-checked, `checked_add`, truncation rejected, no `unsafe`, orientation values filtered to 1-8.
- **Encoder errors never leave partial output files** (in-memory `Vec<u8>` written only on success); HEIF module has `catch_unwind` + RAII libheif handles (no leaks on panic paths).
- **GUI panic surface**: the 51/48/31 unwraps in app.rs/settings.rs/presets are all in `#[cfg(test)]`; progress/ratio helpers guard zero; conversions run in-process via the job API (no argv/shell injection surface; `file_uri` percent-encodes correctly).
- **Supply-chain positives**: jpegxl-src pinned via committed gitlink; zstd tarball checksum-verified at every fetch site; `.sha256` sidecars for release artifacts.
- **Clippy** (root, non-heif feature set): 1 collapsible-if + 2 unused-item warnings — otherwise clean.

---

## Potential improvements

### P0 — stop whole-process death & data loss (small, high-yield patches)
1. Validate/clamp every numeric CLI option at the parse boundary (avif quality/speed, webp quality/alpha-quality, webpanim method, kmin/kmax, gif speed, max-animation-memory ≥ 1) — closes CR-1 and half of M-3.
2. Atomic write path: temp file in the output directory + `fs::rename`, `output == input` detection with a loud warning/hard error, exclusive-create for `KeepExisting` — closes CR-6.
3. Symlink-safe directory walk (`DirEntry::file_type`, canonical-path set, depth cap) — closes CR-2.
4. Real decoder `Limits` (configurable `max_alloc`, default a few hundred MiB) — closes CR-3.
5. EPIPE-safe reporting: `writeln!` + ignore errors, no mutex across I/O — closes H-1.
6. Fail-closed EXIF filtering (strip + warn on re-serialize failure) and mirror HEIF's orientation-failure handling in the JXL input path — closes H-2, M-18.
7. Fix the zpe_stub bounds check (`payload_off >= frames_end` before subtraction) and add a wine execution leg to `pack_binaries` — closes CR-5, M-37.

### P1 — correctness & resource behavior
8. Pre-decode dimension/frame-count probing with `checked_mul` budget enforcement for animations (CR-4), and stream frames to encoders instead of materializing them.
9. Single-owner EXIF policy (delete encoder-local re-resolution) — H-4; wire `--intensity-target` through — H-3.
10. `catch_unwind` around jxl-oxide/image decode paths (M-20) and a dimension guard before the JXL sample allocation (M-21).
11. Thread budgets: call `ThreadBudget::per_files`, pass ravif `threads` explicitly (M-12); register libjxl's resizable parallel runner (M-13).
12. GUI: filter discovered rows from `selection()` (H-6); `finish_job()` on channel disconnect (M-23); off-thread directory scans (H-7); debounced + atomic settings writes (M-27).
13. Ctrl+C escalation to hard exit; optional per-file timeout.

### P2 — performance
14. Eliminate the double per-frame copies in the JXL encoder (H-5) and the one-copy-per-still pattern across encoders (M-16) by borrowing `DynamicImage` buffers.
15. Reporter fast path: skip no-op events, move outcomes instead of cloning, `Relaxed` atomics (M-4, LOWs).
16. GUI: path-indexed event application (M-29), cached sort permutation with precomputed keys (M-30), borrow instead of cloning `RunReport` (M-31), bounded thumbnail queue with invalidation (M-32), async file dialogs (M-25).
17. Metadata: header/chunk-scoped reads instead of whole-file `fs::read`; parse EXIF once per file (M-19).

### P3 — supply chain & packaging hygiene
18. SHA-256 verification for llvm-mingw, libde265, dav1d downloads; serve dav1d from release artifacts (H-8).
19. `permissions: contents: read` top-level + per-job grants; pin all actions to commit SHAs; move `cargo publish` after artifact validation (M-38-40).
20. Digest-pin Docker bases, add a non-root `USER`; mirror `.gitignore` in `.dockerignore` (M-41).
21. PE packer: hash-verified cache, bounded string formatting, frame-count validation in zpack.py, zstd version assertion at pack time (M-33-36).
22. `build.rs`: emit `rerun-if-changed` for jpegxl-src, Cargo.toml/lock (M-42); reconsider global `target-cpu=x86-64-v3` (move to CI/wrappers, or add an `is_x86_feature_detected!` startup guard) (M-43); fix the `ffi.rs:253` doc comment.
23. CI gaps worth closing: run `tools/packer/selftest.sh` for Windows artifacts; enforce `cargo clippy --workspace -- -D warnings` (note: currently impossible without addressing M-10 — give the GUI a `default-features = false` dependency on the core); consider `cargo-deny`/`cargo-audit` for advisory and license gates.
24. Longer term: fuzz the hand-written parsers (`riff.rs`, exif glue) and the zpack stubs' trailer parsing (all four of CR-3/CR-4/CR-5/M-21 are input-shaped bugs a weekend of cargo-fuzz would have found); consider a decode subprocess/sandbox mode for hostile batches; document a supported 16-bit → target-depth matrix (M-11).

---

## Empirical verification log

| Finding | Result |
|---|---|
| CR-1 avif `--quality 0` | reproduced: worker panic, `Error: job worker thread panicked` |
| CR-6 in-place `png → png` | reproduced: `Successful: 0, Skipped: 3`, silent no-op |
| H-1 EPIPE | reproduced: `failed printing to stdout: Broken pipe` panic → job error (release: abort) |
| CR-2 symlink recursion | verified in source (`is_dir()` follows symlinks, no cycle guard) |
| CR-3 `no_limits()` | verified at `src/input/mod.rs:361,417` |
| CR-5 zpe overflow | verified: wrapping `uint64_t` check at `zpe_stub.c:405`; ELF sibling orders correctly |
| H-3 intensity-target | verified: option field read only by `Display`; byte-identical outputs |
| H-6 GUI double-processing | verified: `selection()` includes `discovered`, spec built before `begin_run()` |
| clippy (root pkg, non-heif features) | 3 warnings total (`build.rs:59` collapsible if; 2 unused in feature combo) |
