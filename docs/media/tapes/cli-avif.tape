# cli-avif - AVIF recipe at quality 85 (replaces the legacy avif_cmd_TODO shot).
# Fixture contract: demo/photos/deno.png (single small photo). The encode must
# stay sub-second so the summary's "Time taken" line stays 0 seconds - anything
# else makes the still depend on wall-clock time and sha-unstable (plan 17 §9).
# Hidden setup removes stale outputs so the pre-state is always identical.
Source "_shared.tape"
Output out/cli-avif/frames/
Output out/cli-avif/cli-avif.ascii
Hide
Type "cd .. && rm -rf out-avif" Enter
Ctrl+L
Sleep 200ms
Show
Type `byteshaver "demo/photos/deno.png" avif -q 85 -o out-avif --overwrite-existing`
Enter
# Wait (bare): vhs default pattern />$/ - the shell prompt returning,
# i.e. the conversion finished. Wait+Screen is unusable here: vhs 0.12
# matches against the top of scrollback, not the visible viewport.
Wait
Screenshot out/cli-avif/still.png
Sleep 700ms
