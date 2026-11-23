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

# 1. Terminal-first WRITE mode in live $SHELL (GREIN=1)
Sleep 800ms
Type 'echo "GREIN=$GREIN"'
Enter
Sleep 1.2s

# 2. Switch to MANAGE mode (Ctrl+G) and toggle to Worktrees view (Tab)
Ctrl+G
Sleep 600ms
Tab
Sleep 1s

# 3. Inspect the feature worktree and its live diff preview
Down
Sleep 400ms
Down
Sleep 1.6s

# 4. Open : command palette and live-preview themes via :theme
Type ":"
Sleep 400ms
Type "theme"
Sleep 600ms
Enter
Sleep 800ms
Type "j"
Sleep 700ms
Type "j"
Sleep 800ms
Enter
Sleep 900ms

# 5. Return to Grein view and re-enter WRITE mode in $SHELL
Tab
Sleep 500ms
Ctrl+G
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
