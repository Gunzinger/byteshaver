# cli-jxl - JPEG XL still recipe with explicit quality (optional scene per
# plan 17 section 5.3). 179zrSg.jpg encodes to ~14% in ~3 s on camera.
Source "_shared.tape"
Output out/cli-jxl/frames/
Output out/cli-jxl/cli-jxl.ascii
Hide
Type "cd .." Enter
Ctrl+L
Sleep 200ms
Show
Type `byteshaver "demo/photos/179zrSg.jpg" jxl --quality 85 --overwrite-existing`
Enter
# Wait (bare): vhs default pattern />$/ - the shell prompt returning,
# i.e. the conversion finished. Wait+Screen is unusable here: vhs 0.12
# matches against the top of scrollback, not the visible viewport.
Wait
Screenshot out/cli-jxl/still.png
Sleep 700ms
