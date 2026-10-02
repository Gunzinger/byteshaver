# cli-clean - the `clean` subcommand deleting generated outputs.
# Self-contained: a hidden conversion (unrecorded) produces out-webp/ first,
# then the recorded part shows the actual clean run.
Source "_shared.tape"
Output out/cli-clean/frames/
Output out/cli-clean/cli-clean.ascii
Hide
Type "cd .." Enter
Ctrl+L
Type `byteshaver "demo/screens/*.png" webp -o out-webp --overwrite-existing`
Enter
# Wait for the prompt to return (conversion/clean finished).
Wait
Ctrl+L
Sleep 200ms
Show
Type `byteshaver "out-webp/*.webp" clean`
Enter
# Wait for the prompt to return (conversion/clean finished).
Wait
Screenshot out/cli-clean/still.png
Sleep 700ms
