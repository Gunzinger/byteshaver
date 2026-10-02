# Terminal tapes (VHS scenes)

`.tape` files rendered by [vhs](https://github.com/charmbracelet/vhs) into PNG
frame sequences + text goldens + stills. Authored for plan
`17-media-capture-pipeline` (P4); consumed by `cargo xtask media --only
terminal`.

Tool pins and bootstrap: `../bootstrap-tools.sh` (installs ffmpeg, ttyd, vhs
and the headless Chromium vhs renders with into `target/media-tools/bin`).

## Runner contract (what xtask must provide)

vhs is invoked with `cwd = <stage>/tapes` and each scene expects this stage
layout around it:

```
<stage>/            # stage root - the shell prompt ends up here
  bin/byteshaver    # freshly built release CLI; <stage>/bin prepended to PATH
  demo/             # staged fixtures = byte copy of docs/media/fixtures/
    screens/*.png   # dashboard.png + logo.png (logo carries GPS EXIF)
    photos/*.{jpg,png}
    anim/loading.gif
  tapes/            # this directory; vhs runs from here
  out/              # CREATED BY THE TAPES: scene outputs land here
```

Required environment for vhs (all verified against vhs 0.12.1):

- `PATH` must contain `target/media-tools/bin` (vhs shells out to `ffmpeg`,
  spawns `ttyd`, and resolves its browser via go-rod's `LookPath`, which scans
  `PATH` for a bare `chrome` - the bootstrap symlinks the pinned Chromium
  there).
- `VHS_NO_SANDBOX=1` must be set: vhs only passes `--no-sandbox` to its
  Chromium when this env var is non-empty (`vhs.go`,
  `NoSandbox(os.Getenv("VHS_NO_SANDBOX") != "")`). Containers/CI without user
  namespaces cannot start the browser without it.
- Fonts: run `setup-fonts.sh` once per staged environment
  (`FONT_HOME` defaults to `$HOME/.fonts`; point `FONT_HOME` at the isolated
  stage HOME, or `BH_TAPE_FONT_DIR` at the committed
  `docs/media/fonts/JetBrainsMono` when the stage has no font copy).
  fontconfig picks `~/.fonts` up automatically; `fc-cache -f` is only forced
  when available. (vhs also embeds a JetBrains Mono copy as fallback, but the
  committed fonts are the source of truth.)
- A Chromium needs a writable `HOME` for its cache; xtask's isolated stage
  HOME satisfies this. First launch of the pinned browser takes a few seconds.

Example local run:

```sh
docs/media/bootstrap-tools.sh
export PATH="$PWD/target/media-tools/bin:$PATH" VHS_NO_SANDBOX=1
bash docs/media/tapes/setup-fonts.sh
# ... stage bin/, demo/ per the layout above, then:
cd <stage>/tapes && vhs cli-basic.tape
```

## Output layout (per scene)

Every scene writes into `out/<scene>/` (created next to the tapes, inside the
stage; never committed):

```
out/<scene>/frames/      # PNG frame sequence (primary video source)
out/<scene>/<scene>.ascii
out/<scene>/still.png    # Screenshot directive, taken after the final Wait
```

### Frame naming - IMPORTANT for post-processing

vhs 0.12 writes **two interleaved 5-digit PNG sequences** into the frames dir
(this is NOT the ffmpeg-style single `%04d.png` one might expect):

```
frame-text-%05d.png     # terminal text layer, opaque, 812x348 (canvas-sized)
frame-cursor-%05d.png   # cursor overlay layer, TRANSPARENT background
```

- Numbering starts at `00001` (1-based, `%05d` zero padding, formats defined
  in vhs `video.go`: `textFrameFormat = "frame-text-%05d.png"`,
  `cursorFrameFormat = "frame-cursor-%05d.png"`).
- Both sequences have identical length and must be composited:
  `frame-cursor-NNNNN.png` overlays `frame-text-NNNNN.png` (vhs's own ffmpeg
  chains use exactly this overlay; so should the post pipeline).
- Raw frames are the bare xterm canvas (no margin/bar/border-radius!). The
  WindowBar, MarginFill frame and rounded corners are composited by ffmpeg
  only for `Screenshot` stills and video outputs - frame dirs are pre-composite.
- Frame count = recorded wall seconds x `Set Framerate`; recording spans
  everything after the leading `Hide` block (typing, encodes, Waits, trailing
  Sleep). Counts are reproducible for a scene on the same machine, but a Wait
  that times out inflates them (the failed Wait's whole timeout is recorded),
  so a healthy runner-side retry is worthwhile if a scene comes out long.

### `.ascii` golden semantics (vhs quirk!)

The `.ascii` output is a sequence of 13-row screen snapshots (one block per
executed command, blocks separated by 80-char `-` lines). **vhs 0.12.1's
buffer read walks the scrollback buffer from its top, not the visible
viewport**: once a scene scrolls (terminal is 64x13 at the pinned geometry),
the final block shows the *head* of the session (typed command + first output
lines) instead of the final screen. Goldens therefore capture the command
text and encoder banner reliably, but NOT the trailing summary numbers unless
a scene's whole session fits 13 rows (e.g. `cli-clean`'s visible part does).
Treat goldens as advisory input only - see plan section 9 (D6).

## Tape conventions

- Every scene starts with `Source "_shared.tape"` (quoted; unquoted paths
  with `_` fail to lex) - settings+`Require` only, see the header there. VHS
  splices sourced commands in place and forbids nested `Source` and
  `Output`s inside the sourced file. `Source` resolves relative to the vhs
  process cwd, which is why the runner contract pins `cwd = <stage>/tapes`.
- Hidden setup: the first command block after the settings/outputs is wrapped
  in `Hide` ... `Show` (vhs pre-executes a leading Hide block before recording
  starts). It performs `cd ..` (stage root) + `Ctrl+L` so neither the cd nor
  the stage path ever appear on camera.
- Idempotency: conversions use `--overwrite-existing` so re-runs against a
  dirty stage produce the same short summary (skipped outputs would add
  "Preexisting" lines and push the summary off the 13-row screen).
- Completion sync: a **bare `Wait`** after `Enter`. vhs's default wait pattern
  is `/>$/` - the shell prompt returning, i.e. the conversion finished. Do
  NOT use `Wait+Screen /<summary regex>/`: it matches against the top of
  scrollback (same quirk as the goldens) and misses scrolled-away summaries.
  `Wait+Line` only inspects the single line under the cursor, which the
  summary lines flash by too quickly to be reliable. `Set WaitTimeout 45s`
  (in `_shared.tape`) is the safety net for slow encodes; under pathological
  load a Wait can still time out - the runner may want to retry a failed
  scene once.
- Every tape ends with `Screenshot` followed by `Sleep 700ms`. The Screenshot
  only ARMS a capture - the frame is grabbed on the recorder's next tick
  (~33 ms at 30 fps); as the literal last command, vhs's shutdown can cancel
  the recorder before that tick and the still is then silently missing (seen
  reproducibly on one scene). The trailing Sleep guarantees the tick; the
  still still shows the settled final screen.

## Scenes

| tape | command shown | measured (local, loaded box) | video @30fps | notes |
|------|---------------|------------------------------|--------------|-------|
| `cli-basic.tape` | `demo/screens/*.png` -> webp, `-o out-webp` | ~6-11 s wall | ~4 s (122 frames) | hero candidate; still + video |
| `cli-avif.tape` | `demo/photos/z.jpeg` -> avif `-q 85` | ~11-14 s wall | ~8 s (240 frames) | single small photo keeps speed-3 inside the <= 20 s budget |
| `cli-exif.tape` | `demo/screens/logo.png` -> webp + EXIF filter | ~9 s wall | ~6 s (191 frames) | shows `exif: N kept, M dropped (except gps,GPSInfo)` |
| `cli-anim.tape` | `demo/anim/loading.gif` -> webp-anim | ~7 s wall | ~4.5 s (134 frames) | single tiny gif, instant encode |
| `cli-clean.tape` | hidden conversion, then `out-webp/*.webp` -> `clean` | ~9 s wall | ~2.5 s (75 frames) | self-contained; visible part fits 13 rows, golden accurate |
| `cli-jxl.tape` | `demo/photos/179zrSg.jpg` -> jxl `--quality 85` | ~11 s wall | ~8 s (239 frames) | optional scene per plan 5.3; ~14% ratio |

(wall time includes ~4 s of vhs browser/tty startup per invocation; frame
counts are wall-clock-proportional, see the capture note above.)

## Fixture contracts (inputs the tapes require)

The stage's `demo/` tree is a byte copy of `docs/media/fixtures/` (see that
README for the per-file spec). Constraints the scenes rely on:

- `demo/screens/*.png`: keep it to small screenshots (currently
  `dashboard.png` + `logo.png`) so the webp batch encodes in ~1 s.
  `logo.png` MUST carry GPS EXIF - the `cli-exif` scene prints the
  `exif: N kept, M dropped (except gps,GPSInfo)` policy line against it.
- `demo/photos/z.jpeg` is referenced by name (`cli-avif`): a single ~172 KiB
  JPEG. avif speed 3 costs ~6.5 s on it; adding files or pixels to this
  scene's input blows the <= 20 s scene budget. Do not glob wider than this
  file in the avif tape.
- `demo/photos/179zrSg.jpg` is referenced by name (`cli-jxl`).
- `demo/anim/loading.gif`: the `cli-anim` input; keep it tiny.

## VHS 0.12.1 quirks discovered (verified locally)

1. `Output <dir>/` renames an internal frame dir into place at exit via
   `os.Rename`, which fails SILENTLY (exit 0) in two cases: when the parent
   dir does not exist, AND when the target frames dir already exists (e.g.
   from a previous run) - the second case leaves the STALE previous frames in
   place and discards this run's frames, while `.ascii`/`Screenshot` still
   update. The runner must guarantee a fresh `out/` per run (a fresh stage
   does; local iteration: `rm -rf out/<scene>` first). Pairing the frames dir
   with the `.ascii` output in the same parent covers the missing-parent
   case; a frames-dir-only tape produces nothing.
2. `Wait+Screen` matches the top of scrollback, not the viewport (see above).
3. `Source` requires a quoted path; bare `_shared.tape` fails with a lexer
   error. Nested `Source` is rejected; `Output` lines inside a sourced tape
   are dropped.
4. `Type` strings have no escape sequences; use backtick strings to show
   double quotes on screen: `` Type `byteshaver "pattern" webp` ``.
5. The shell is vhs's own bash config (`--noprofile --norc`) with a purple
   `>` prompt - by design, don't expect the stage path in the prompt.
6. Rendering goes through a headless Chromium (go-rod), not just ttyd+ffmpeg:
   the pinned browser + `VHS_NO_SANDBOX=1` are hard requirements (bootstrap
   script handles both).
7. Frames-dir outputs are raw canvas layers (two sequences); stills/videos
   get the margin/bar/radius compositing. Geometry: `Set Width/Height` is the
   COMPOSITED size (stills are exactly 1000x560); raw text frames are
   812x348 at the pinned font/margin.
