# media fixtures - pinned demo inputs

These files are the **committed, byte-pinned inputs** of the README media
pipeline (plan 17 §5.4). Capture scenes convert them on camera; they are
never edited by hand and never produced by a conversion. Generated outputs
(experiment results, converted copies) live exclusively in
`target/media-stage/` and are not committed.

## Tree

```
fixtures/
  photos/             6 real photographs, byte-copies from examples/
                      (z.jpeg, 4D71QrY.jpg, 179zrSg.jpg, deno.png,
                       5613.png, AQ5MKa0.png - a ~73 KB .. ~1.4 MB mix
                       of 3 JPEG + 3 PNG so globs, queue tables and the
                       progress summary look natural on camera)
  screens/            synthetic UI-like stills (procedurally drawn)
    dashboard.png     480x320, no EXIF
    logo.png          640x360, carries EXIF: Make="byteshaver",
                      Model="fixture-gen", DateTimeOriginal, and a GPS
                      lat/long (Munich) - the `--exif filter
                      --exif-except gps` scene strips it on camera.
                      No Orientation tag at all, so no decoder rotates it.
  anim/               one fixture per animated input format
    loading.gif       animated GIF, 8 frames, 240x180, 100 ms/frame
    sparkle.png       APNG, 6 frames, 320x240, 100 ms/frame
                      (note: APNG content, standard .png extension)
    spinner.webp      animated WebP, 8 frames, 240x180, 100 ms/frame,
                      lossy quality 80
```

`jxl/` is intentionally absent: encoding a JXL fixture would require
libjxl at generation time. The `cli-jxl` scene (optional, plan 17 §5.3)
converts one of the photos instead.

## Regenerating

```bash
cargo media gen-fixtures     # or: cargo xtask gen-fixtures
```

Generation is deterministic: photos are byte-copies, screens/animations
are drawn with integer-math procedural code (no time, no randomness) and
encoded with fixed settings. Two runs on the same toolchain produce
byte-identical files - verify with `git status` (must show no changes)
or `sha256sum` before and after. Commit the results; they are inputs.

The generator lives in `xtask/src/fixtures.rs`; the per-file drawing code
is deliberately boring and commented. If you need a new fixture, add it
there (never by hand) so it stays reproducible.

## Related

- Pipeline layout contract (scene dirs, `%04d` frames, finals):
  the module documentation of `xtask/src/post.rs`.
- Publishing, manifest and `?v=` cache busting: `xtask/src/manifest.rs`,
  `xtask/src/readme.rs`, plan 17 §8.
