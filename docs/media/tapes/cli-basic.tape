# cli-basic - hero scene: batch PNG -> WebP conversion with progress bar and
# the "Encode statistics" summary. Small, fast fixtures keep the video short.
#
# Runs with cwd = <stage>/tapes (xtask contract); `cd ..` (hidden) puts the
# prompt at the stage root so commands read `byteshaver "demo/..."`.
Source "_shared.tape"
Output out/cli-basic/frames/
Output out/cli-basic/cli-basic.ascii
Hide
Type "cd .. && rm -rf out-webp" Enter
Ctrl+L
Sleep 200ms
Show
Type `byteshaver "demo/screens/*.png" webp -o out-webp --overwrite-existing`
Enter
# Wait (bare): vhs default pattern />$/ - the shell prompt returning,
# i.e. the conversion finished. Wait+Screen is unusable here: vhs 0.12
# matches against the top of scrollback, not the visible viewport.
Wait
Screenshot out/cli-basic/still.png
Sleep 700ms
