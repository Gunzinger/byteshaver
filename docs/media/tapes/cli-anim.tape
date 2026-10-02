# cli-anim - single small animated GIF converted to animated WebP
# (webp-anim keeps all frames). loading.gif is a tiny deterministic fixture.
Source "_shared.tape"
Output out/cli-anim/frames/
Output out/cli-anim/cli-anim.ascii
Hide
Type "cd .." Enter
Ctrl+L
Sleep 200ms
Show
Type `byteshaver "demo/anim/loading.gif" webp-anim --overwrite-existing`
Enter
# Wait (bare): vhs default pattern />$/ - the shell prompt returning,
# i.e. the conversion finished. Wait+Screen is unusable here: vhs 0.12
# matches against the top of scrollback, not the visible viewport.
Wait
Screenshot out/cli-anim/still.png
Sleep 700ms
