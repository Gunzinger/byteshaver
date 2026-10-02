# cli-avif - AVIF recipe at quality 85 (replaces the legacy avif_cmd_TODO shot).
# Fixture contract: demo/photos/z.jpeg (single ~172 KiB photo). avif speed 3
# costs ~6 s on it; larger or more photos would blow the <= 20 s scene budget.
Source "_shared.tape"
Output out/cli-avif/frames/
Output out/cli-avif/cli-avif.ascii
Hide
Type "cd .." Enter
Ctrl+L
Sleep 200ms
Show
Type `byteshaver "demo/photos/z.jpeg" avif -q 85 --overwrite-existing`
Enter
# Wait (bare): vhs default pattern />$/ - the shell prompt returning,
# i.e. the conversion finished. Wait+Screen is unusable here: vhs 0.12
# matches against the top of scrollback, not the visible viewport.
Wait
Screenshot out/cli-avif/still.png
Sleep 700ms
