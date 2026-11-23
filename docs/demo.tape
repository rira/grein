# Reproducible VHS demo tape for grein
# Run from the repository root: vhs docs/demo.tape

Output docs/demo.gif

Require grein
Require git

Set Shell "bash"
Set FontSize 14
Set Width 1280
Set Height 720
Set Padding 16
Set Theme "TokyoNight"
Set TypingSpeed 45ms

Hide
Type 'DEMO_TMP=$(mktemp -d) && export GREIN_CONFIG_DB="$DEMO_TMP/grein.db"'
Enter
Type 'git -C "$DEMO_TMP" init -q -b main && git -C "$DEMO_TMP" config user.name demo && git -C "$DEMO_TMP" config user.email demo@example.com'
Enter
Type 'mkdir -p "$DEMO_TMP/.git/info" && echo ".grein/" >> "$DEMO_TMP/.git/info/exclude"'
Enter
Type 'echo "pub fn grein_demo() -> i32 { 1 }" > "$DEMO_TMP/lib.rs" && git -C "$DEMO_TMP" add . && git -C "$DEMO_TMP" commit -q -m init'
Enter
Type 'git -C "$DEMO_TMP" worktree add -q -b feat/demo-wt "$DEMO_TMP/.grein/worktrees/feat-demo" HEAD'
Enter
Type 'echo "pub fn grein_demo() -> i32 { 42 }" > "$DEMO_TMP/.grein/worktrees/feat-demo/lib.rs"'
Enter
Type 'cd "$DEMO_TMP" && clear && grein'
Enter
Sleep 1.5s
Show

# 1. Terminal-first WRITE mode in live $SHELL (GREIN=1) and Vim-first scrollback (Alt+K / Alt+J)
Sleep 800ms
Type 'echo "GREIN=$GREIN" && seq 1 20'
Enter
Sleep 1.2s
Alt+k
Sleep 700ms
Alt+j
Sleep 700ms

# 2. Enter MANAGE mode (Ctrl+G), create [n]ew root agent (n) and spawn child [s]hell (s)
Ctrl+G
Sleep 600ms
Type "n"
Sleep 900ms
Ctrl+G
Sleep 500ms
Type "s"
Sleep 900ms
Ctrl+G
Sleep 500ms
Type "k"
Sleep 400ms
Type "s"
Sleep 900ms

# 3. Cycle sibling threads in WRITE mode (Alt+H / Alt+L)
Alt+h
Sleep 700ms
Alt+l
Sleep 700ms

# 4. Return to MANAGE (Ctrl+G), fuzzy-filter Help (? -> /), delete a thread (d -> y), and switch to Worktrees view (Tab)
Ctrl+G
Sleep 500ms
Type "?"
Sleep 500ms
Type "/"
Type "hide"
Sleep 700ms
Enter
Sleep 400ms
Escape
Sleep 300ms
Escape
Sleep 400ms
Type "d"
Sleep 500ms
Type "y"
Sleep 600ms
Tab
Sleep 1s

# 5. Inspect the feature worktree, attached threads, and its live diff preview
Down
Sleep 400ms
Down
Sleep 1.6s

# 6. Open : command palette and live-preview themes across the 16 swatch-tagged presets via :theme
Type ":"
Sleep 400ms
Type "theme"
Sleep 600ms
Enter
Sleep 800ms
Type "j"
Sleep 450ms
Type "j"
Sleep 450ms
Type "j"
Sleep 450ms
Type "j"
Sleep 450ms
Type "j"
Sleep 700ms
Enter
Sleep 900ms

# 7. Create a [n]ew root agent in the worktree (auto-returns to Grein + WRITE · AGENT) & use Composer word autocomplete
Type "n"
Sleep 900ms
Type "refac"
Sleep 600ms
Tab
Type " @lib"
Sleep 600ms
Tab
Sleep 700ms
Ctrl+C
Sleep 400ms

# 8. Return to MANAGE (Ctrl+G), jump to root 1 (gg), and press Enter to enter WRITE in $SHELL
Ctrl+G
Sleep 400ms
Type "gg"
Sleep 400ms
Enter
Sleep 700ms
Type 'git status -sb'
Enter
Sleep 1.5s

Hide
Ctrl+G
Sleep 250ms
Type "q"
Sleep 250ms
Type "y"
Sleep 400ms
Type 'cd / && rm -rf "$DEMO_TMP"'
Enter
