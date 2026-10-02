# Shared settings sourced by every scene tape via: Source "_shared.tape"
# MUST stay at the top of each scene: VHS ignores settings applied after the
# first non-setting command. This file therefore contains ONLY `Set` and
# `Require` commands (no Output, no shell actions; VHS also forbids nesting
# Source, so scenes may not Source anything else).
#
# Canonical theme copy: ../theme/vhs.json (keep in sync - `Set Theme` accepts
# inline JSON or a built-in theme name only, not a file path).
Set Shell "bash"
Set FontFamily "JetBrains Mono"
Set FontSize 18
Set Framerate 30
Set TypingSpeed 45ms
Set CursorBlink false
Set Margin 16
Set MarginFill "#0d0f12"
Set WindowBar "Colorful"
Set BorderRadius 8
Set Width 1000
Set Height 600
Set Theme { "name": "byteshaver", "black": "#1a1d24", "red": "#ef6487", "green": "#5eca89", "yellow": "#fdd877", "blue": "#65aef7", "magenta": "#aa7ff0", "cyan": "#43c1be", "white": "#f1f5f9", "brightBlack": "#5c6672", "brightRed": "#ff7d9c", "brightGreen": "#7ce69f", "brightYellow": "#ffe49b", "brightBlue": "#8cc4fa", "brightMagenta": "#c39bfa", "brightCyan": "#6ad8d5", "brightWhite": "#ffffff", "background": "#16181d", "foreground": "#e2e8f0", "selection": "#2d3640", "cursor": "#5eca89" }
# Wait caps the default 15s so encodes on slow CI runners don't abort the
# scene (Wait returns as soon as the pattern matches; this is only the cap).
Set WaitTimeout 45s
Require byteshaver
