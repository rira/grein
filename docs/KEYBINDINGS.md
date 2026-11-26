# Grein Keybindings Reference

> Complete, mode-by-mode reference for every shortcut, slash command, palette action, mouse gesture, and `:bind` customization option in **grein**.
> See also: [`README.md`](../README.md) · [`FEATURES.md`](./FEATURES.md) · [`ARCHITECTURE.md`](./ARCHITECTURE.md)

---

## Mode Model & Layout

**grein** operates with a strict 2-mode focus model between the right-hand workspace pane and the left-hand sidebar:

| Mode | Status Pill | Active Pane | Default Focus |
| :--- | :--- | :--- | :--- |
| **`WRITE`** | `WRITE · SHELL` or `WRITE · AGENT` | Embedded `>_` Terminal PTY or Agent `Composer` | Primary workspace input |
| **`MANAGE`** | `MANAGE · GREIN` or `MANAGE · WORKTREES` | `ᛇ Grein` context tree or `⎇ Worktrees` list | Sidebar tree navigation & actions |

On startup, `TerminalGuard` enables raw mode, alternate screen, bracketed paste, mouse capture, and (when supported by the host terminal) `KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES` so `Shift+Enter` and modified navigation chords are distinguished cleanly.

---

## 1. Global Gateway Keys

These bindings are evaluated in `WRITE` mode and also checked in `MANAGE` mode (`Ctrl+G` exits `MANAGE` back to `WRITE`, and `Alt+1..9` / `Alt+Z` / `Alt+H/L` / `Alt+K/J` work seamlessly in both modes).

On **macOS**, Grein defaults to the `macos` keymap preset (when `keybinding.preset` is unset) so status bar and `?` Help hints highlight zero-config `Ctrl+G` workflows that never collide with Nordic/European `Option` symbol composition—while keeping `Alt+` and `Shift+Arrows` active whenever your terminal sends them (switchable via `:keymap standard` / `:keymap macos`).

| Default Key | Action Name (`:bind`) | Scope & Behavior |
| :--- | :--- | :--- |
| `Ctrl+G` | `enter_manage_mode` | Toggle `WRITE` ↔ `MANAGE`. Opening `MANAGE` auto-unhides a zoomed sidebar (`sidebar_hidden = false`); exiting `MANAGE` focuses the selected thread's `Composer` or `Terminal` (or returns to `ᛇ Grein` if in `⎇ Worktrees`). Combine as `Ctrl+G` then `1..9` to jump to a root thread, or `Ctrl+G` then `z` to toggle sidebar zoom symmetrically. Cannot be unbound or left with zero bindings. |
| `Alt+Z` *(or `Ctrl+G z`)* | `toggle_zoom` | Toggle sidebar visibility (`Sidebar hidden (copy zoom)` ↔ `Sidebar shown`). `Ctrl+G` then `z` is symmetric in both directions: it hides the sidebar and returns to `WRITE`, and when `Ctrl+G` just unhid a hidden sidebar, pressing `z` immediately returns to `WRITE` while keeping the sidebar visible. |
| `Alt+1` … `Alt+9` *(or `Ctrl+G 1..9`)* | *(built-in)* | Jump directly to depth-0 root thread `1..9`. `Ctrl+G` followed by plain `1..9` switches root and immediately returns to `WRITE`. `Alt+1..9` explicitly checks `Alt && !Ctrl` so `AltGr` (`Control+Alt`) chords never trigger root switching. |
| `Alt+K` / `Alt+J` | `scroll_up` / `scroll_down` | Scroll active target: in `MANAGE · WORKTREES`, scrolls `⎇ Diff` (`±10` lines); on a `>_` Terminal, scrolls PTY scrollback (`+5`/`-5` lines); on an Agent thread, scrolls the transcript (`±10` lines). In `WRITE · AGENT` and `MANAGE`, plain `PageUp` / `PageDown` also scroll without needing `Alt+` (`WRITE · SHELL` forwards `PageUp` / `PageDown` straight to the PTY). |
| `Alt+H` / `Alt+L` | `prev_sibling` / `next_sibling` | Cycle to the previous or next sibling thread sharing the same `parent_thread_id` (wraps around). In `MANAGE · GREIN`, `Ctrl+P` / `Ctrl+N` and `[` / `]` step siblings without `Alt+`. |
| `Shift+↑` / `Shift+↓` | `tree_up` / `tree_down` | Step to the previous or next sibling thread at the current `parent_thread_id` depth (wraps around) and switch `sidebar_view` to `ᛇ Grein`. |
| `Shift+←` | `tree_parent` | Ascend to the active thread's `parent_thread_id`. Records the current thread as `last_visited_child` on that parent so `Shift+→` returns to it. |
| `Shift+→` | `tree_child` | Descend into the active thread's `last_visited_child` (if still valid) or first direct child, and automatically unfold any collapsed ancestor subtrees. |

### `alternate_screen` PTY Passthrough

When `FocusedPane::Terminal` is active on a live PTY running an alternate-screen TUI (`nvim`, `vim`, `tmux`, `helix`, `lazygit`, `htop`, `less`), `active_pty_is_alt_screen()` becomes `true`. Grein automatically passes the following actions straight through to the PTY instead of intercepting them:

- `ScrollUp` (`Alt+K`) and `ScrollDown` (`Alt+J`)
- `PrevSibling` (`Alt+H`) and `NextSibling` (`Alt+L`)
- `TreeUp` (`Shift+↑`), `TreeDown` (`Shift+↓`), `TreeParent` (`Shift+←`), and `TreeChild` (`Shift+→`)

`Ctrl+G` (`enter_manage_mode`) and `Alt+Z` (`toggle_zoom`) remain active as Grein gateways so you can always leave an embedded editor or zoom the pane.

---

## 2. `WRITE` Mode — Agent Composer (`WRITE · AGENT`)

When an Agent thread is active and focused, keystrokes first pass through active modals, global gateway bindings, the `/worktree ` popup (if open), `Ctrl+C` / `Esc`, and active inline autocomplete controls before reaching the multiline readline editor.

### Submit, Cancel & Multiline Navigation

| Key | Behavior |
| :--- | :--- |
| `Enter` | Submit prompt, `/slash` command, or `!command` inline shell execution |
| `Shift+Enter`, `Ctrl+J`, `Alt+Enter`, `Ctrl+Enter` | Insert newline without submitting |
| `↑` / `↓` | Move cursor up/down across wrapped visual lines while preserving horizontal column (`composer_desired_col`) |
| `PageUp` / `PageDown` | Scroll active thread transcript up/down (`±10` lines, without `Shift`) |
| `Ctrl+C` | Cancel active turn if busy (`streaming`/`executing`/`thinking`); otherwise clear non-empty input buffer; if already empty, display hint (`Use Ctrl+G then q (or /quit) to exit`) |
| `Esc` | Close `/worktree ` popup if open; else cancel active turn if busy; else clear active right-pane mouse selection |

### Readline Editing & Clipboard

| Key | Behavior |
| :--- | :--- |
| `Ctrl+A` / `Home` | Move to start of current line |
| `Ctrl+E` / `End` | Dismiss ghost-text completion if visible (`Ctrl+E`); otherwise move to end of current line |
| `Ctrl+B` / `Ctrl+F` | Move backward / forward one character (`←` / `→` also move one char) |
| `Alt+B` / `Ctrl+←` / `Alt+←` | Move backward to previous word boundary |
| `Alt+F` / `Ctrl+→` / `Alt+→` | Move forward to next word boundary |
| `Backspace` / `Ctrl+D` / `Del` | Delete character backward (`Backspace`) or forward (`Ctrl+D` / `Del`) |
| `Ctrl+W` / `Alt+Backspace` | Kill previous word into the Composer `kill_ring` |
| `Alt+D` / `Ctrl+Del` / `Alt+Del` | Kill next word into the Composer `kill_ring` |
| `Ctrl+U` | Kill from cursor to start of line (or join with previous line if already at line start) |
| `Ctrl+K` | Kill from cursor to end of line (or join with next line if already at line end) |
| `Ctrl+Y` | Accept ghost-text completion if visible; otherwise yank `kill_ring` at cursor |
| `Ctrl+V` | Paste from system clipboard (`\r\n` and `\r` normalized to `\n`) |
| `AltGr` (`Ctrl+Alt+char`) | Inserts non-control UTF-8 characters directly (for example Swedish `@ £ $ { } [ ] \ \| ~ € µ`) |

### `!command` Inline Shell Prefix

Prefixing a `WRITE · AGENT` (`Composer`) submission with `!` (for example `!cargo test` or `!git status -sb`) runs the command via `PtyShellExecutor` and renders a collapsible `ShellExecution` transcript card without leaving `WRITE · AGENT`:

- **PTY & `$SHELL` Parity:** Executes via the user's configured `$SHELL -c` (falling back to `/bin/sh`) inside a `40×120` PTY session, inheriting your terminal's `PATH` and supporting `$SHELL` syntax (pipes, redirections, globs, subshells, and `zsh`/`bash` expressions) plus color TTY detection (without breaking downstream pipes via `CLICOLOR_FORCE`).
- **Non-Interactive I/O Guardrails:** Binds `stdin` to `/dev/null` and detaches `/dev/tty` via `setsid` so commands never hang on hidden terminal prompts.
- **Stateless in Thread `cwd`:** Each `!command` invocation is **stateless** in the thread's `cwd` (`cd` and shell variables do not persist across runs). Use `/wt` or `/worktree` to switch directories, or open a `>_` embedded terminal tab with `s`/`S` in `MANAGE` (or `/term`) for persistent stateful shells, interactive prompts/TUIs, or interactive rc aliases.
- **Timeouts, Truncation & Card Controls:** Enforces a **30 s** process-group timeout (terminating child pipelines cleanly and surfacing a timeout notice in `stderr`), a **64 KiB per-stream** truncation cap (`stdout` and `stderr`), and collapsing/expanding cards via `o` in `MANAGE` mode.

### `/worktree ` & `/wt ` Popup

Typing `/wt ` or `/worktree ` (single-line) opens the floating **Worktrees** selector directly above the Composer, prioritizing worktrees from the active repository:

| Key | Behavior |
| :--- | :--- |
| `↓` / `↑` / `Shift+Tab` | Cycle through `+ New isolated worktree` and discovered worktrees |
| `Tab` | Insert `/worktree <path>` (or `/worktree`) into Composer and close popup |
| `Enter` | Immediately allocate a new worktree (`EnsureThreadWorktree`) or switch the active thread to the selected worktree (`SwitchThreadWorktree`) |
| `Esc` | Dismiss the popup |

### Tier 1 / 2 / 3 Inline Ghost-Text Autocomplete

When `composer_autocomplete` is enabled (default `on`), **grein** computes up to 12 ranked ghost-text suggestions at the cursor:

1. **Tier 1 (`SlashOrWorkspace`):** Built-in & ACP-advertised `/slash` commands (at command start—e.g. typing `/ag` uniquely completes `/agent`), `@file` workspace paths, workspace paths/basenames (`>= 2` chars, up to 5,000 files via `git ls-files` or directory walk), and Git branches/worktrees.
2. **Tier 2 (`TranscriptOrHistory`):** Col-0 full-line recent prompt recall from SQLite history (`token_start == 0`), plus identifiers extracted from the active transcript (last 256 KB) and recent prompt words (`>= 2` chars).
3. **Tier 3 (`DictionaryWord`):** Embedded **6,878-word** English & software engineering wordlist (`>= 3` ASCII alphabetic chars, preserving `lowercase`, `Title`, or `UPPERCASE`).

| Default Key | Action Name (`:bind`) | Behavior |
| :--- | :--- | :--- |
| `Tab` / `Ctrl+Y` | `accept_completion` | Insert active ghost-text suffix at cursor |
| `Shift+Tab` / `Ctrl+N` | `next_completion` | Cycle to next completion match |
| `Ctrl+P` | `prev_completion` | Cycle to previous completion match |
| `Ctrl+E` | `dismiss_completion` | Dismiss ghost text until next edit |

Toggle and persist autocomplete with `/autocomplete [on|off|status]` or `:autocomplete`.

---

## 3. `WRITE` Mode — Embedded `>_` Terminal (`WRITE · SHELL`)

Embedded terminal sessions spawn `$SHELL` (falling back to `/bin/sh`) with `GREIN=1`, `TERM=xterm-256color`, `COLORTERM=truecolor`, and inherited multiplexer environment variables (`TMUX`, `TMUX_PANE`, `KITTY_WINDOW_ID`, `ZELLIJ`, `WEZTERM_PANE`, etc.) scrubbed cleanly.

### Live PTY Key Handling & Allowlist

While the PTY child process is running:

- **Intercept Allowlist:** Only active `write` keymap chords (defaults: `Ctrl+G`, `Alt+Z`, `Alt+K/J`, `Alt+H/L`, `Shift+↑/↓/←/→`, minus the `alternate_screen` passthrough set when applicable) and `Alt+1..9` are intercepted by Grein.
- **Native Shell Passthrough:** `Tab`, `Shift+Tab`, `PageUp`, `PageDown`, `Ctrl+Y`, `Ctrl+N`, `Ctrl+P`, `Ctrl+E`, `Ctrl+T`, `Ctrl+Q`, `Ctrl+F`, `Ctrl+C`, `Esc`, `?`, and all `F1..F12`/navigation keys pass straight to `vt_input::encode_key_event` (respecting `application_cursor` `CSI`/`SS3` mode, sending `\x1b[5~` / `\x1b[6~` for `PageUp` / `PageDown`).
- **Swedish & International `AltGr`:** Printable non-control, non-ASCII-alphabetic characters arriving with `CONTROL | ALT` (such as `@`, `£`, `$`, `{`, `}`, `[`, `]`, `\`, `|`, `~`, `€`, `µ`) encode directly as raw UTF-8 bytes instead of being mangled into `Esc+Ctrl` sequences.
- **`Ctrl+V` Raw `\x16` Preservation:** `FocusedPane::Terminal` does **not** intercept `Ctrl+V` as a clipboard paste shortcut; it forwards raw `0x16` (`\x16`) to the PTY so shell `quoted-insert` and Vim visual-block mode work natively. Paste into a live PTY using terminal bracketed paste (`\x1b[200~...\x1b[201~`) or middle-click.

### Exited PTY Controls

When the shell exits (`✓ exit 0`, `✗ exit <code>`, or `✓ exited` in the pane header and status bar):

| Key | Behavior |
| :--- | :--- |
| `Enter` | Restart the PTY in the thread's persisted `cwd` at the current pane dimensions |
| `Esc` | Switch focus to `MANAGE` (`FocusedPane::Tree`) |
| `Ctrl+G` | Toggle to `MANAGE` (`FocusedPane::Tree`) |

---

## 4. `MANAGE` Mode — `ᛇ Grein` & `⎇ Worktrees` Sidebars

Pressing `Tab` in `MANAGE` mode toggles between `ᛇ Grein` (`MANAGE · GREIN`) and `⎇ Worktrees` (`MANAGE · WORKTREES`).

### Shared `MANAGE` Keys (Both Views)

| Key | Behavior |
| :--- | :--- |
| `Tab` | Switch sidebar view: `ᛇ Grein` ↔ `⎇ Worktrees` |
| `Ctrl+G` | Return to `WRITE` mode (also switches `⎇ Worktrees` back to `ᛇ Grein`) |
| `z` / `Alt+Z` | Toggle sidebar zoom symmetrically (`z` hides sidebar and returns to `WRITE`; if `Ctrl+G` just unhid a zoomed sidebar, immediate `z` keeps it visible and returns to `WRITE`) |
| `1` … `9` / `Alt+1..9` | Plain `1..9` jumps directly to depth-0 root `1..9`, switches to `ᛇ Grein`, and returns to `WRITE` (2-keystroke switch via `Ctrl+G 1..9`; out-of-range digits are no-ops; text inputs still receive digits). `Alt+1..9` switches root while staying in `MANAGE`. |
| `Alt+H` / `Alt+L` | Cycle previous/next sibling thread |
| `Ctrl+U` / `Ctrl+D`, `PageUp` / `PageDown` | Scroll right-pane preview up/down half-page (transcript / `⎇ Diff`, or `±5` PTY scrollback rows) |
| `Ctrl+B` / `Ctrl+F` | Scroll right-pane preview up/down full-page (transcript / `⎇ Diff`, or `±10` PTY scrollback rows) |
| `Alt+K` / `Alt+J` | Scroll right-pane preview (`10` lines transcript / `⎇ Diff`, or `±5` PTY scrollback rows) |
| `:` | Open Sidebar Command Palette (`:`) |
| `?` | Open Help modal |
| `q` / `Ctrl+C` | Quit Grein (opens confirmation modal if live terminals or busy agent turns are active) |

### `ᛇ Grein` Sidebar Keys

| Key | Behavior |
| :--- | :--- |
| `j` / `↓` and `k` / `↑` | Move selection down / up (requests PTY snapshot when selecting a terminal) |
| `h` / `←` | Fold selected unfolded parent node, or jump selection to parent node/thread |
| `l` / `→` | Unfold selected folded node, or step into its first child |
| `Ctrl+P` / `[` and `Ctrl+N` / `]` | Step to previous (`Ctrl+P` / `[`) or next (`Ctrl+N` / `]`) sibling thread at the current `parent_thread_id` depth while staying in `MANAGE · GREIN` (works without `Alt+` or `Shift+Arrows`) |
| `Space` | Toggle subtree collapse/expand on selected node |
| `H` / `L` | Collapse all (`H`) or expand all (`L`) parent branches |
| `J` / `K` | Jump to next (`J`) or previous (`K`) depth-0 root thread |
| `gg` / `G` | Jump to first (`gg`) or last (`G`) visible item in tree |
| `Enter` | Switch to selected thread and enter `WRITE` mode (or activate selected heading anchor) |
| `Esc` | Clear active `/` transcript search if present; otherwise activate selected node into `WRITE` mode |
| `n` | Create a new root Agent thread (note: when `/` transcript search has an active query, `n` steps to the next search match) |
| `f` | Fork a child Agent branch from the selected thread |
| `A` | Open **`Select Agent`** modal targeting the selected or active thread (`Enter` new sibling, `s` switch provider, `d` set default) |
| `s` / `S` | Spawn a child `>_` Terminal under selected thread (`s`) or a new root `>_` Terminal (`S`) |
| `w` | Ensure/allocate an isolated worktree (or non-Git shadow snapshot) for the selected thread |
| `o` | Collapse/expand all tool-call and shell-execution cards in the selected thread's transcript |
| `/` | Open forward transcript search (`n` next match, `N` prev match, `Esc` clear) |
| `r` | Begin inline `Rename:` prompt (empty input clears custom name back to auto context) |
| `m` | Join/merge selected child branch into its parent (`DecisionLandmark` + `[joined]` badge) |
| `d` | Prompt to delete selected context (`PendingThreadDelete`, blocked if only 1 context remains) |
| `y` | Copy latest assistant reply or terminal screen to clipboard |

### `⎇ Worktrees` Sidebar & Live `⎇ Diff` Preview Keys

The `⎇ Worktrees` view displays discovered worktrees grouped by repository (with the active repository and `(primary)` pinned first), a synced `Target: <thread> · <M> repos · <N> wt` footer, and a live colorized `⎇ Diff` preview in the right pane (`Repo:`, `Path:`, `Attached:`, and up to 400 lines of `git status` + `git diff HEAD`, or shadow directory diff).

| Key | Behavior |
| :--- | :--- |
| `j` / `↓` and `k` / `↑` | Move selection down / up and refresh `⎇ Diff` preview |
| `gg` / `G` | Jump to first (`gg`) or last (`G`) visible row |
| `Space` | Toggle fold of the current repository group (from header or entry) |
| `h` / `←` | Fold expanded repository header, or jump from worktree entry up to its repository header |
| `l` / `→` | Unfold collapsed repository header, or step into its first worktree entry |
| `H` / `L` | Collapse all (`H`) or expand all (`L`) repository groups |
| `Enter` | On worktree entry: attach active thread (`SwitchThreadWorktree`) and return to `ᛇ Grein` in `WRITE` mode. On repo header: toggle fold |
| `n` | Create a `[n]ew` fresh root Agent thread in the selected worktree's path and return to `ᛇ Grein` in `WRITE · AGENT` |
| `s` | Spawn a `>_` Terminal under the active thread in the selected worktree's path and return to `ᛇ Grein` in `WRITE · SHELL` |
| `w` | Allocate an isolated worktree for the active `Target:` thread |
| `p` | Prune stale Git worktree registrations (`git worktree prune`) |
| `r` | Refresh worktree discovery list and force-reload `⎇ Diff` preview |
| `y` | Copy selected worktree path to clipboard |
| `d` | Prompt to delete selected non-primary worktree (`PendingWorktreeDelete`; refuses `(primary)`, prompts force-delete if dirty, resets attached threads to repo root) |
| `/` | Activate live worktree filter bar (`Enter`/`↑`/`↓` lock filter, `Esc`/`Ctrl+C` clear & exit, `Ctrl+V` paste) |
| `Esc` | Clear non-empty filter query, or return to `ᛇ Grein` in `WRITE` mode |

---

## 5. Mouse Selection, Copy & Paste

Mouse events are handled when no modal is open (`ShowHelp` intercepts wheel scroll `±3` lines).

| Gesture | Target Pane | Behavior |
| :--- | :--- | :--- |
| **Left click** | Sidebar (`Tree`) | Focus `MANAGE`, select clicked thread/worktree row, and load PTY snapshot / diff preview |
| **Left click** | `Composer` | Focus `WRITE · AGENT` (`Composer`) and clear pane selection |
| **Left drag** | Right pane (`Transcript`, `Terminal`, `WorktreeDiff`) | Pane-scoped cell highlight clamped strictly inside `right_inner` (zero sidebar or `│` border bleed). Freezes a live PTY snapshot during drag, snaps wide CJK/emoji cells, strips trailing spaces, rejoins soft-wrapped lines, and **auto-copies on mouse release** |
| **Double-click** | Right pane | Select word run (`alphanumeric`/`_`, whitespace, or symbol run) and copy to clipboard (`400 ms` click window) |
| **Triple-click** | Right pane | Select logical line and copy to clipboard |
| **`Alt` + drag / click** | Live `Terminal` PTY | Bypasses `vt100` PTY mouse forwarding (`pty_screens` `mouse_protocol_mode`) so pane-scoped Grein selection always works inside `nvim`, `htop`, or `tmux` |
| **Mouse wheel** | Right pane / Sidebar / Help | Scrolls `±3` lines in `Transcript`, `Terminal` scrollback, `WorktreeDiff`, or Help modal; steps selection `±1` row in `MANAGE` sidebar |
| **Middle-click** | `Composer` / `Terminal` / global | Reads X11/Wayland primary selection (`read_primary`), falling back to clipboard (`read_clipboard`), and pastes into the focused input (`Terminal` click focuses `Terminal`; `Composer` click focuses `Composer`) |

### Clipboard & Copy Commands

- **Copy Backends (`ChainedClipboardDriver`):** Attempts native backends first—`pbcopy` (macOS), `wl-copy` → `xclip` → `xsel` (Wayland), or `xclip` → `xsel` (X11)—with automatic `OSC 52` fallback (automatically wrapped in `\x1bPtmux;...\x1b\\` when `$TMUX` is set).
- **Keyboard Copy:** `y` (`MANAGE · GREIN`) or `/copy` copies the latest assistant reply or PTY screen; `/copy code` copies the last fenced code block; `/copy all` copies the full transcript; `Alt+Z` / `:zoom` hides the sidebar (`copy zoom`) for distraction-free copying.

---

## 6. Slash Commands, `:` Palette, Modals & `:bind` Customization

### Slash Commands & `:` Sidebar Palette

Commands work both from Composer (`/command`) and the `MANAGE` Sidebar Command Palette (`:command`, where a scrollable 5-row dropdown includes built-in commands plus dynamic `:<provider>` sibling-spawn entries for every registered agent; `Tab` completes the highlighted suggestion, `↑`/`Ctrl+P` and `↓`/`Ctrl+N` cycle suggestions, `Ctrl+U`/`Ctrl+W`/`Ctrl+A`/`Ctrl+E`/`Ctrl+V` edit, and `Esc`/`Ctrl+C` or `Backspace` on empty closes).

| Command | Aliases | Arguments & Effect |
| :--- | :--- | :--- |
| `/help` | `/?`, `:help` | Open Help modal with live `/` fuzzy filter (`A, :, /, ?, z, q` & `/agent /model /mode /effort` rows) |
| `/zoom` | `:zoom` | Toggle sidebar zoom (`Alt+Z`) |
| `/copy` | `:copy` | `/copy` (last reply or PTY screen), `/copy code` (last fenced code block), `/copy all` (full transcript) |
| `/commands` | — | Fetch and display ACP-advertised agent slash commands (`available_commands_update`) |
| `/agent` | `:agent`, `:<provider> [title]` | Bare `/agent` / `:agent` opens **`Select Agent`** modal. With `<provider> [title]` (or `:<provider> [title]` in palette), spawns a sibling thread inheriting `cwd` and sharing `parent_thread_id` (`None` at root depth 0, `Some(parent)` in sub-trees), then focuses `WRITE · AGENT` |
| `/model` | — | Open Model picker modal, or pass `<query>` (`[--default\|default] <query>`) to apply directly |
| `/mode` | — | Open Mode picker modal, or pass `<query>` (`[--default\|default] <query>`) to apply directly |
| `/thinking` | `/effort` | Open Thinking/Reasoning level picker modal, or pass `<query>` (`[--default\|default] <query>`) |
| `/root` | `/new`, `:new` | Create a new root Agent thread with optional `[title]` |
| `/fork` | `/branch`, `/grein`, `:fork` | Fork the active/selected thread with optional `[title]` |
| `/rename` | `:rename` | Set custom name (`/rename <title>`); in `:rename` with no args starts inline `Rename:` prompt |
| `/join` | `/merge`, `:join` | Synthesize a `DecisionLandmark` onto parent thread (`[summary]` optional) and mark child `[joined]` |
| `/delete` | `/close` | Delete active/selected thread or `[id\|title]` (refuses if only 1 context remains) |
| `/term` | `/terminal`, `/zsh`, `/sh`, `/shell`, `/bash`, `:term` | Spawn a child `>_` PTY terminal with optional `[cwd]` (supports `~` and quoted paths) |
| `/worktree` | `/wt`, `:worktree` | With no args: allocate branch worktree (or shadow fallback). With `[query]`: switch to matching worktree or open filtered `⎇ Worktrees` |
| `/worktrees` | — | Open `⎇ Worktrees` sidebar view with optional `[filter]` |
| `/prune` | `:prune` | Prune stale Git worktree registrations (`git worktree prune`) |
| `/autocomplete` | `:autocomplete` | Toggle or set Composer inline autocomplete (`on` \| `off` \| `status`) |
| `/theme` | `:theme` | Open live-preview Theme Picker modal, or apply `/theme <preset>` (`tokyo-night`, `catppuccin-mocha`, `nord`, `gruvbox-dark`, `rose-pine`, `kanagawa`, `dracula`, `everforest`, `one-dark`, `solarized-dark`, `github-dark`, `monokai`, `catppuccin-latte`, `gruvbox-light`, `rose-pine-dawn`, `ansi`) |
| `/sidebar` | `:sidebar` | Configure sidebar layout: `dynamic [min max]`, `fixed <cols\|pct%>`, `min <10..50%>`, `max <min..60%>`, `overflow <on\|off>`, or shorthand `<cols>` / `<pct%>` |
| `/keymap` | `:keymap` | Show or switch keymap hint preset: `macos` (`mac`, `leader`) or `standard` (`default`, `linux`), persisted in `grein.db` (`keybinding.preset`) |
| `/bind` | `:bind` | Bind a `WRITE` action: `:bind <action> <chords>` (persists to `grein.db`) |
| `/unbind` | `:unbind` | Reset an action back to its built-in default: `:unbind <action>` (removes `grein.db` override) |
| `/keybinds` | `/binds`, `:keybinds` | Display current bindings (pass `custom` to show only overridden actions) |
| `/quit` | `/q`, `/exit`, `:quit`, `:q`, `:exit` | Quit Grein immediately |

### Inline Prompts & Modals

- **Inline Rename (`r` in `MANAGE` / `:rename`) & Search (`/` in `MANAGE`):**
  - Rename supports `Enter` (save; empty clears custom name), `Esc`/`Ctrl+C` (cancel), `Ctrl+A`/`Home`, `Ctrl+E`/`End`, `Ctrl+U`, `Ctrl+W`, `←`/`→`, `Backspace`/`Del`, `Ctrl+V`, and `AltGr`.
  - Transcript search supports `Enter` (confirm/lock), `Esc`/`Ctrl+C` (clear), `Backspace`, `Ctrl+V`, `AltGr`, and `n`/`N` once confirmed.
- **Help Modal (`?` / `/help` / `/?`):**
  - Press `/` to open Smith-Waterman fuzzy filtering across keys, descriptions, section titles, and semantic keywords (e.g. `hide` matches `Alt+Z`/`z`).
  - While filtering: type to match, `Backspace`/`Ctrl+U`/`Ctrl+W` edit, `Enter` locks the filter, `↑`/`Ctrl+P`/`↓`/`Ctrl+N`/`PgUp`/`PgDn`/`Home`/`End` scroll, `Esc`/`Ctrl+C` clears filter.
  - When locked/unfiltered: `j`/`↓`/`k`/`↑`, `Ctrl+D`/`PgDn` (`+10`), `Ctrl+U`/`PgUp` (`-10`), `Ctrl+F` (`+20`), `Ctrl+B` (`-20`), `g`/`Home`, `G`/`End`, mouse wheel (`±3`); `Esc` clears non-empty filter or closes; `q`/`?`/`Enter`/`Ctrl+C` closes.
- **Confirmation Modals (Permission, Delete Thread, Delete Worktree, Quit Confirm):**
  - `y` / `Y`: Approve / Confirm.
  - `n` / `N` / `Esc` / `Ctrl+C`: Deny / Cancel.
- **Theme Picker (`:theme` / `/theme`):**
  - `↑`/`k`, `↓`/`j`, `Home`, `End`: Live-preview each of the 16 presets with 5-dot swatches and tone tags (`dark` / `light` / `term`).
  - `Enter`: Save preset and clear per-slot overrides; `Esc` / `q` / `Ctrl+C`: Revert to previous preset and overrides.
- **Agent Config Picker (`/model`, `/mode`, `/thinking`, `/effort`):**
  - `↑`/`k`, `↓`/`j`: Move selection.
  - `Tab` / `BackTab` / `d`: Toggle scope between `[This branch only]` and `[Default for new branches]`.
  - `Enter`: Apply config; `Esc` / `q` / `Ctrl+C`: Close.
- **`Select Agent` Modal (`A` in `MANAGE`, `:agent`, or `/agent`):**
  - Displays `Active: <provider> · Default: <provider>`, `● ready` / `not found` status with `(active)` / `(default)` / `(active, default)` tags, an `Install:` hint when an unavailable provider is highlighted, and sidebar/status-bar badges (`[<provider>]` in `ᛇ Grein` rows, `<provider> · <mode> · <model>` in the status bar).
  - `↑` / `k` (`K`) and `↓` / `j` (`J`): Move selection up / down.
  - `Home` / `End`: Jump to first / last provider.
  - `Enter`: Spawn a new sibling agent thread using the selected provider (inherits `cwd`, shares `parent_thread_id`, closes modal, and focuses `WRITE · AGENT`). Blocked with an install hint or `PATH` notice if the provider is `not found`.
  - `s` (`S`): Switch the target agent thread's provider (resets its active ACP session when changed while preserving conversation history; refuses on `>_` shell threads).
  - `d` (`D`): Set the selected provider as global default (persists `[ui].default_agent_provider` in `UserPrefsStore` `grein.db`; keeps modal open).
  - `Esc` / `q` (`Q`) / `Ctrl+C`: Close modal (`↑/↓/j/k Select · Enter New sibling · s Switch · d Default · Esc/q Close`).

### Keymap Presets (`:keymap macos` / `:keymap standard`)

Use `:keymap <preset>` or `/keymap <preset>` (pass no arguments to view the active preset) to switch between `macos` and `standard`, persisted in SQLite (`grein.db`, `keybinding.preset`):

- **`macos` (aliases: `mac`, `leader`) — Default on macOS when `keybinding.preset` is unset:**
  - Status bar and `?` Help modal show `Ctrl+G`-first hints (`Ctrl+G manage · Ctrl+G z sidebar · PgUp/PgDn scroll · Ctrl+G 1..9 root · /help`, plus `Ctrl+G Ctrl+P/N, [/]` in Help) without `Alt+` or `Shift+Arrow` clutter.
  - **All `Alt+` and `Shift+Arrows` bindings stay active** whenever your terminal sends them, while `Ctrl+G` workflows work out of the box without changing macOS `Option`-key behavior—preserving Swedish/Nordic/European `Option` symbols (`@`, `[`, `]`, `{`, `}`, `\`, `|`, `~`).
- **`standard` (aliases: `default`, `linux`) — Default on Linux/non-macOS when unset:**
  - Status bar and `?` Help modal display `Alt+Z`, `Alt+K`/`Alt+J`, `Alt+H`/`Alt+L`, `Alt+1..9`, and `Shift+↑/↓/←/→` hints.

### Customizing Keybindings (`:bind`, `:unbind`, `grein.db`)

Use `:bind <action> <chords>` or `/bind <action> <chords>` to bind one or more comma-separated chords (`KeyChord::parse`):

- **Modifiers:** `ctrl` (`control`, `c`), `alt` (`meta`, `opt`, `option`, `m`), `shift` (`s`) — combined with `+` (for example `ctrl+alt+p` or `ctrl++`).
- **Special Keys:** `tab`, `backtab`, `enter` (`return`, `ret`), `esc` (`escape`), `space` (`spc`), `backspace` (`bs`), `delete` (`del`), `insert` (`ins`), `pageup` (`pgup`), `pagedown` (`pgdn`, `pgdown`), `home`, `end`, `up`, `down`, `left`, `right`, `f1`…`f12`, or any single character.
- **Special RHS Values:** `default` resets to built-in default; `none` or `unbound` removes the binding (rejected for `enter_manage_mode`).
- **Conflict Resolution:** Binding a chord automatically removes that chord from any other conflicting action unless doing so would leave `enter_manage_mode` empty.

| Canonical Action (`as_str`) | Aliases (`BindableAction::from_str`) | Built-in Default | Description |
| :--- | :--- | :--- | :--- |
| `enter_manage_mode` | `manage`, `tree`, `toggle_tree`, `manage_mode` | `ctrl+g` | Toggle `WRITE` ↔ `MANAGE` sidebar |
| `toggle_zoom` | `zoom` | `alt+z` | Toggle sidebar zoom |
| `scroll_up` | `page_up`, `scrollback_up` | `alt+k` | Scroll terminal/transcript/`⎇ Diff` up |
| `scroll_down` | `page_down`, `scrollback_down` | `alt+j` | Scroll terminal/transcript/`⎇ Diff` down |
| `prev_sibling` | `previous_sibling`, `sibling_prev` | `alt+h` | Switch to previous sibling thread |
| `next_sibling` | `sibling_next` | `alt+l` | Switch to next sibling thread |
| `tree_up` | `tree_prev`, `sibling_up` | `shift+up` | Step to previous sibling thread in tree |
| `tree_down` | `tree_next`, `sibling_down` | `shift+down` | Step to next sibling thread in tree |
| `tree_parent` | `tree_up_level`, `parent_thread` | `shift+left` | Ascend to parent thread in tree |
| `tree_child` | `tree_down_level`, `child_thread` | `shift+right` | Descend to child thread in tree |
| `toggle_worktrees` | `worktrees`, `wt_view` | `unbound` | Toggle `ᛇ Grein` ↔ `⎇ Worktrees` view |
| `jump_terminal` | `terminal`, `term` | `unbound` | Open or jump to terminal |
| `new_agent` | `create_root`, `new_root`, `new` | `unbound` | Create new root agent thread |
| `fork_thread` | `fork`, `branch` | `unbound` | Fork active/selected thread |
| `worktree` | `ensure_worktree`, `wt` | `unbound` | Ensure thread worktree |
| `command_palette` | `palette`, `cmd` | `unbound` | Open sidebar command palette |
| `help` | `show_help` | `unbound` | Open help modal |
| `toggle_cards` | `cards` | `unbound` | Toggle tool/shell cards |
| `copy_reply` | `copy` | `unbound` | Copy last reply or terminal screen |
| `quit` | `exit` | `unbound` | Quit Grein |
| `accept_completion` | `complete`, `completion_accept` | `tab,ctrl+y` | Accept inline Composer completion |
| `next_completion` | `completion_next` | `backtab,ctrl+n` | Next inline Composer completion |
| `prev_completion` | `previous_completion`, `completion_prev` | `ctrl+p` | Previous inline Composer completion |
| `dismiss_completion` | `completion_dismiss` | `ctrl+e` | Dismiss inline Composer completion |

Custom keybindings are stored in `~/.config/grein/grein.db` (or `$GREIN_CONFIG_DB`) under the `keybinding` namespace and can be exported/imported via `grein settings export --only keybinding` and `grein settings import`.

---

## 7. Terminal & Multiplexer Notes (macOS, Colors & `tmux`)

- **Zero-Config Stock macOS & Nordic/European Keyboards:**
  - Grein works out of the box on stock Apple `Terminal.app` and Nordic/European keyboard layouts **without** enabling *Option as Meta* or adding custom escape maps:
    - `Ctrl+G 1..9`: Switch directly to root thread `1..9` and return to `WRITE`.
    - `Ctrl+G z`: Toggle sidebar zoom symmetrically in both directions.
    - `Ctrl+G` then `Ctrl+P`/`Ctrl+N` (or `[`/`]`): Step siblings in `MANAGE · GREIN`.
    - `PageUp` / `PageDown`: Scroll transcript in `WRITE · AGENT` and preview in `MANAGE` (while passing through natively in `WRITE · SHELL`).
  - Keeping macOS `Option` in normal composition mode preserves native `@`, `[`, `]`, `{`, `}`, `\`, `|`, and `~` symbols, and on macOS the `macos` keymap preset is selected automatically at startup unless overridden via `:keymap standard`.
- **Diagnosing Terminal Keys & Color Capabilities (`grein keys` & `grein doctor`):**
  - Run `grein keys` (or `grein keys --no-enhance` to disable Kitty/CSI-u `DISAMBIGUATE_ESCAPE_CODES` probing) in any interactive TTY to inspect live `crossterm` events (`code`, `modifiers`, `kind`, `state`, resolved `chord`, and matching `action`), plus `TERM`, `TERM_PROGRAM`, and `COLORTERM`. It automatically flags stripped `Shift+Arrow` escapes and macOS `Option` symbol composition without manual `cat -v` guesswork. Press `Ctrl+C` twice or `q` twice to exit.
  - Run `grein doctor [--json]` to verify `TERM_PROGRAM` and detected color support (`truecolor` vs `256-color fallback`, automatically downsampling RGB theme colors on non-truecolor TTYs such as pre-Tahoe Apple `Terminal.app`).
- **Optional macOS `Option` / `Meta` & `Terminal.app` `Shift+Arrows` Mapping:**
  - If you *also* want single-chord `Alt+` shortcuts, enable **Use Option as Meta key** (Terminal.app → *Settings → Profiles → Keyboard*) or set Left/Right Option to **Esc+** (iTerm2 → *Profiles → Keys*)—though note this replaces macOS `Option`-composed characters on that key.
  - Modern macOS terminals (**Ghostty**, **iTerm2 3.5+**, **Kitty**, **WezTerm**, **Alacritty**) send `Shift+Arrows` out of the box. Pre-Tahoe `Terminal.app` strips `Shift` from arrow escapes unless mapped manually under *Terminal → Settings → Profiles → Keyboard* (`+`):
    - `Shift+Up` → `\033[1;2A` · `Shift+Down` → `\033[1;2B` · `Shift+Left` → `\033[1;2D` · `Shift+Right` → `\033[1;2C`
- **Running Grein inside `tmux`:**
  - `ChainedClipboardDriver` detects `$TMUX` and automatically wraps `OSC 52` payloads in DCS passthrough (`\x1bPtmux;\x1b\x1b]52;c;...\x07\x1b\\`). Ensure `set -g allow-passthrough on` is enabled in `tmux.conf` if relying on `OSC 52` without a native clipboard binary.
  - Add both `set -s extended-keys on` **and** `set -g extended-keys-format csi-u` (alongside `escape-time 10`) in `tmux.conf` if `Shift+Enter` or `Shift+Arrows` are swallowed by `tmux`—`tmux 3.5+` defaults to `extended-keys-format xterm`, which emits `CSI 27;...~` sequences that `crossterm 0.28.1` ignores.
- **Running `tmux` inside Grein's embedded `>_` PTY:** Embedded shells scrub `TMUX` and `TMUX_PANE` on spawn and trigger `alternate_screen` passthrough automatically when you launch nested `tmux`, `nvim`, or `helix` inside a Grein terminal.
