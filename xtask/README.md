# xtask

Repository automation for byteshaver, in the plain `cargo xtask` tradition:
no external task runner, just a workspace member that cargo builds and runs
via the aliases in `.cargo/config.toml` (`cargo xtask <args>` or the shorter
`cargo media <args>`, both `--release`). Currently it hosts the README media
capture pipeline of plan 17 (`docs/plans/17-media-capture-pipeline.md`): one
command regenerates every screenshot/video asset of the README from
committed, code-defined inputs and publishes them with cache-busted links.

## Command surface

```text
cargo media                  full pipeline: stage -> terminal -> gui -> post -> publish
cargo media --check          regenerate into target/ only, compare sha256 against
                             docs/img/ + manifest, table + exit 1 on drift
cargo media --dry-run        everything except writes outside target/
cargo media --only <list>    restrict to stages (stage|terminal|gui|post|publish)
                             and/or scenes (tape stems / gui scene ids)
cargo media --skip-video     stills only, no frame-sequence work
cargo media --gif            additionally emit GIF variants of animations
cargo media readme           only the README ?v= rewrite + link lint pass
cargo media deps             external tool check (vhs, ttyd, ffmpeg) + binaries
cargo media gen-fixtures     regenerate docs/media/fixtures/ deterministically
```

## How it fits together

`target/media-stage/` is the scratch workspace, rebuilt from scratch each
run: fixtures are copied to `demo/`, the freshly built release CLI to
`bin/`, and every child process (vhs, ffmpeg, byteshaver, the GUI capture
binary) runs under an isolated env (`HOME`/`XDG_*` under the stage, `PATH`
prepended with the staged bin dir) so captures are hermetic and never read
or write the developer's real config. The terminal (VHS) and GUI (kittest)
capture engines are separate phases; their plumbing - scene discovery over
`docs/media/tapes/*.tape` and `bh-gui-capture --list`, staging, env, tool
lookup - is already in place here, so integration later is a small diff.

Post-processing turns capture artifacts into final assets (see the layout
contract in the `post.rs` module docs): ffmpeg does the 2x->1x lanczos
downscale and the animated WebP encodes, and the byteshaver binary itself
produces the still WebPs - the pipeline dogfoods its own product. The
publish step hashes each final, bumps its `version` in
`docs/media/manifest.json` only when bytes changed, rewrites every
`docs/img/<asset>?v=<n>` reference in the READMEs accordingly (defeating
GitHub's camo cache) and hard-fails on dangling image references.

## Conventions

- Errors: `anyhow` with context; external tool failures carry the command
  line and the tail of stderr.
- Determinism where it matters: fixtures are byte-stable (procedural
  drawing, byte-copies), stills are hash-comparable, videos intentionally
  are not - their freshness is tracked via the manifest's `generated_by`
  (plan 17 §9).
- Tools are looked up in `MEDIA_TOOLS_PATH` (colon-separated extra bin
  dirs) before `PATH`, so a pinned toolchain can win over system installs.
