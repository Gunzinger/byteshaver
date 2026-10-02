# cli-exif - EXIF policy demo: keep everything except GPS data while
# converting a PNG that carries GPS EXIF. The "exif: N kept,
# M dropped" policy line is part of the visible output.
Source "_shared.tape"
Output out/cli-exif/frames/
Output out/cli-exif/cli-exif.ascii
Hide
Type "cd .." Enter
Ctrl+L
Sleep 200ms
Show
Type `byteshaver "demo/screens/logo.png" webp --exif filter --exif-except gps,GPSInfo --overwrite-existing`
Enter
# Wait (bare): vhs default pattern />$/ - the shell prompt returning,
# i.e. the conversion finished. Wait+Screen is unusable here: vhs 0.12
# matches against the top of scrollback, not the visible viewport.
Wait
Screenshot out/cli-exif/still.png
Sleep 700ms
