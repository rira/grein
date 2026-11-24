# Grein Features Guide

This guide covers every implemented workflow and subsystem in **grein** for users and contributors. For a full key-by-key map see [`KEYBINDINGS.md`](./KEYBINDINGS.md); for internal ports and persistence schemas see [`ARCHITECTURE.md`](./ARCHITECTURE.md).

---

## 1. `ᛇ Grein` DAG Sessions

Conversations in **grein** form a Git-like **Pure-Tree Directed Acyclic Graph (DAG)** forest of immutable turn nodes (`ConversationNode`) and explicit branch heads (`Thread`):

- **Independent Roots (`ᛉ`) & Child Branches (`├─` / `└─`):**
  - Top-level roots (`parent_thread_id = None`) are indexed `1..9` in the sidebar and start with completely isolated context. Create a fresh root agent via `n` (`MANAGE`), `Ctrl+G n`, or `/root` (`/new`).
  - Child branches fork from the active or selected agent thread via `f` (`MANAGE`) or `/fork` (`/branch`, `/grein`), recording an explicit `parent_thread_id` and `parent_node_id`.
- **Sibling & Root Isolation:**
  - When compiling prompts, a forked branch inherits only its ancestor trunk up to `parent_node_id` plus its own turns. Sibling branches and separate roots are strictly excluded—no global blackboard scratchpad leaks across tasks.
- **Subtree Collapsing & Tree Navigation:**
  - Collapse or expand individual subtrees in `MANAGE` mode with `Space`, or use `h`/`←` (fold current node or jump to parent) and `l`/`→` (unfold or step into first child while staying in `MANAGE`).
  - `H` collapses all branches; `L` expands all branches. Collapsed rows render `▸ (+N)` showing hidden descendants.
  - From `WRITE` mode, `Shift+↑`/`Shift+↓` (and `Alt+H`/`Alt+L`) step between siblings at the current `parent_thread_id` level, `Shift+←` ascends to the parent thread, `Shift+→` descends to the last-visited (or first) child, and `Alt+1..9` jumps directly to root `1..9`.
- **Unified `[custom name] auto context` Naming:**
  - Rename any context inline with `r` (`MANAGE`) or `/rename <title>` (an empty rename clears the custom name back to auto-only).
  - **Agents (`ᛉ`):** Display their ACP session auto-title (`Title`), or `[custom name] Title` when renamed (suppressing generic placeholders such as `Main`, `Root N`, `Branch N`, or `Untitled`).
  - **Terminals (`>_`):** Display their live `cwd` (`~/projects` or `worktree: <slug>`), or `[custom name] ~/projects` when renamed.
- **Manual `/join` & Decision Synthesis:**
  - Pressing `m` (`MANAGE`) or running `/join [summary]` (`/merge`) commits a synthesized `DecisionLandmark` node directly onto the parent branch's `active_leaf_id` and badges the child `[joined]`.
  - If the parent thread holds an active ACP session, the joined summary is delivered append-only as `[Joined Context]` on its next turn—preserving LLM prompt KV caches.
- **Live Subagent Strip & Tool Cards:**
  - Active subagents render in a dedicated `Subagents` strip above the status bar (up to 5 visible rows with `●`/`✓`/`✖`, `Agent(type: role)`, optional model, live activity, and elapsed timer, plus `+N more` overflow).
  - Tool-call and shell-execution transcript cards collapse/expand cleanly via `o` in `MANAGE`.

---

## 2. `⎇ Worktrees` & Non-Git Shadow Fallback

Worktree isolation is opt-in per thread via `w` (`MANAGE`) or `/worktree` (`/wt`).

### Branch-Based Allocation & Primary-Root Anchoring
- **Non-default branches** (anything other than `main`, `master`, or detached `HEAD`): allocates `<primary_repo_root>/.grein/worktrees/<branch-slug>` on branch `<branch>-wt`, auto-suffixing `-2`, `-3`, … on directory, registered-worktree, or branch-ref collisions.
- **Default / detached (`main`, `master`, or detached `HEAD`):** allocates `<primary_repo_root>/.grein/worktrees/<thread_id>` on branch `worktree-<thread_id>`.
- **Primary-root anchoring & exclude hygiene:** Allocation always resolves `<primary_repo_root>` via `git worktree list --porcelain` so linked worktrees never nest `.grein/worktrees/` inside another worktree, and `.grein/` is automatically appended to git `info/exclude`.

### Non-Git Shadow Snapshot Fallback
Outside Git repositories (or when `git` is unavailable on `$PATH`), `grein-workspace` falls back to ephemeral **Shadow Snapshots** under `<primary_dir>/.grein/shadow/<thread_id>`:
- Clones files via copy-on-write reflinks (`cp --reflink=auto` on Linux, falling back to plain file copy—never hard links) while excluding `.git` and `.grein`.
- Computes unified diffs against the primary directory with an embedded line-diff engine and merges verified additions, modifications, and deletions back cleanly.

### Composer `/worktree` Autocomplete & `⎇ Worktrees` Sidebar
- **Composer `/wt ` / `/worktree ` popup:** Typing `/wt ` or `/worktree ` opens a live popup of discovered worktrees prioritizing the active repository (`↑`/`↓` navigate, `Tab` completes, `Enter` allocates or switches).
- **`⎇ Worktrees` view (`Tab` in `MANAGE` or `/worktrees [filter]`):**
  - Lists discovered worktrees grouped by repository (active repository first, `(primary)` pinned first), showing `●` for the active worktree, `*N` dirty counts, `[N]` attached thread badges, `/` live filtering, and `h`/`l`/`H`/`L` repo folding.
  - **Synced `Target:` footer:** Automatically tracks the active/selected thread (`Target: <thread> · <M> repos · <N> wt`).
  - **Direct launch & attach actions:** `Enter` attaches the target thread to the selected worktree, `n` creates a `[n]ew` fresh Agent root thread directly inside that worktree, and `s` spawns a shell there—all three (plus `Ctrl+G` and `Esc`) return automatically to `ᛇ Grein` in `WRITE` mode.
  - **Live `⎇ Diff` Preview:** Right pane renders `Repo:`, `Path:`, human-readable `Attached:` thread display names (never raw UUIDs), and colorized `git status --short --branch`, `--stat`, and `git diff HEAD` output (up to 400 lines, scrollable with `Ctrl+U/D`, `Ctrl+F/B`, `Alt+K/J`, or mouse wheel).
  - **Safe cleanup & ACP preservation:** `p` (`/prune`) prunes stale worktree registrations; `r` refreshes; `y` copies the worktree path; `d` safely removes non-primary worktrees (confirming when dirty or attached and moving attached threads back to `repo_root`). Attaching an agent thread already targeting the primary worktree (including default `cwd = None`) preserves its active ACP session without resetting.

---

## 3. Embedded `>_` PTY Terminals

Launching `grein` opens directly into `WRITE · SHELL` inside `$SHELL`:

- **Environment & VT100 Fidelity:** Child shells receive `GREIN=1` with inherited multiplexer variables (`TMUX`, `TMUX_PANE`, `STY`, etc.) scrubbed cleanly. The VT100 screen engine renders 24-bit RGB and indexed colors, bold/italic/underline/inverse styles, and wide CJK/emoji continuation cells.
- **1,000-Line Scrollback Ring Buffer:** Non-alternate-screen output scrolls into a 1,000-row ring buffer (`Alt+K`/`Alt+J`, `Ctrl+U/D`/`Ctrl+F/B` in `MANAGE`, or mouse wheel); typing into a scrolled-back PTY snaps cleanly back to the live prompt.
- **Terminal Query Scanner & `OSC 7` CWD Sync:** Responds to `DSR` (`CSI 5n`, `CSI 6n`, `CSI ?6n`), `DA1` (`CSI c`), `DA2` (`CSI >c`), and `OSC 10`/`11` foreground/background color queries, while syncing live working directories via `OSC 7` with OS process polling fallback (`/proc/<pid>/cwd` on Linux, `lsof` on macOS).
- **Swedish & International `AltGr` + Key Passthrough:** `CONTROL | ALT` printable non-alphabetic characters (e.g., `@£${}[]\~|€`) encode directly as UTF-8 into both PTY and Composer/palette inputs. In `WRITE · SHELL`, native shell completions (`Tab`, `Shift+Tab`, `Ctrl+Y/N/P/E/T/Q/F`, `Esc`, `?`) pass straight to `$SHELL`, and when an alternate-screen app (`nvim`, `htop`, `less`, `tmux`) is active, `Alt+K/J/H/L` and `Shift+Arrows` also pass through untouched.
- **Lifecycle Controls:** Border and status bar show `● live`, `✓ exit 0`, or `✗ exit <code>`; once exited, `Enter` restarts the PTY and `Esc` switches to `MANAGE`.

---

## 4. Composer & Autocomplete

- **Visual Multiline & Readline Editing:** `Shift+Enter`, `Ctrl+J`, or `Alt+Enter` inserts a newline; `Enter` submits prompts, `/slash` commands, or `!cmd` inline shell commands. `↑`/`↓` navigate wrapped visual lines while preserving horizontal column (`composer_desired_col`). Full readline bindings (`Ctrl+A/E/B/F/D/K/U/W`, `Alt+←/→/B/F/D/Backspace`, `Ctrl+Y` kill-ring yank, `Ctrl+V` clipboard paste, `Ctrl+C` cancel/clear) are supported.
- **3-Tier Inline Ghost-Text Word Autocomplete (`WRITE · AGENT`):**
  1. **Tier 1 (`SlashOrWorkspace`):** Built-in & ACP `/slash` commands, `@file` paths, workspace files (up to 5,000 files via `git ls-files` or directory walk), and Git branches/worktrees.
  2. **Tier 2 (`TranscriptOrHistory`):** Col-0 full-line recent prompt recall from SQLite history, plus identifiers from the active transcript (last 256 KB) and prompt history words (`>= 2` chars).
  3. **Tier 3 (`DictionaryWord`):** Embedded **6,878-word** English & software engineering wordlist (`>= 3` ASCII alphabetic chars, matching typed `lower`/`Title`/`UPPER` casing).
- **Completion Controls:** `Tab` or `Ctrl+Y` accepts, `Shift+Tab` or `Ctrl+N` cycles next, `Ctrl+P` cycles previous, and `Ctrl+E` dismisses active ghost text (or moves to line end when inactive). Toggle and persist via `/autocomplete [on|off|status]` or `:autocomplete`.

---

## 5. Themes, Fuzzy Help & Copy/Paste

- **16 Curated Themes with Live Preview:** Open `:theme` / `/theme` (or pass `/theme <name>`) to preview **12 dark** (`tokyo-night`, `catppuccin-mocha`, `nord`, `gruvbox-dark`, `rose-pine`, `kanagawa`, `dracula`, `everforest`, `one-dark`, `solarized-dark`, `github-dark`, `monokai`), **3 light** (`catppuccin-latte`, `gruvbox-light`, `rose-pine-dawn`), and **1 terminal** (`ansi`) presets with 5-dot swatches (`accent`, `accent_secondary`, `success`, `warning`, `error`) and tone tags. All 24 palette slots support hex (`#rrggbb`/`#rgb`) and ANSI/indexed overrides persisted in `grein.db`.
- **Fuzzy-Filtered Help Modal (`?` or `/help` / `/?`):** Press `/` inside the Help modal for Smith-Waterman-style fuzzy filtering across keys, descriptions, sections, and semantic keywords (e.g., `"hide"` matches `Alt+Z`/`z`). Press `Enter` to lock the filter and scroll with `j`/`k`; press `Esc` once to clear the filter or again to close.
- **Pane-Scoped Mouse Selection & Copy/Paste:**
  - Dragging in the right pane selects strictly inside `Transcript`, `Terminal`, or `WorktreeDiff` inner coordinates—eliminating sidebar and `│` border bleed—while rejoining soft-wrapped lines, stripping trailing padding, snapping wide PTY cells, and freezing live PTY snapshots during drag.
  - **Double-click** selects a word; **triple-click** selects the logical line; **`Alt+drag`** overrides PTY mouse forwarding so selection always works inside `nvim`/`htop`/`tmux`.
  - Release auto-copies via `pbcopy` / `wl-copy` / `xclip` / `xsel` with automatic `OSC 52` fallback (`\x1bPtmux;...` wrapped under `tmux`).
  - **Middle-click** pastes X11 primary selection (falling back to clipboard); `Ctrl+V` and bracketed paste work in Composer, palette, rename, and search prompts.
  - Keyboard copy & zoom: `y` (`MANAGE`) or `/copy` copies the latest assistant reply or PTY screen, `/copy code` copies the last fenced code block, `/copy all` copies the full transcript, and `Alt+Z` / `:zoom` toggles sidebar visibility (`Sidebar hidden (copy zoom)`).

---

## 6. Headless CLI, ACP Agents & Global Preferences

- **Multi-Agent ACP Discovery:** Inspects `$GREIN_ACP_BIN`, `PATH` (`agy`, `claude`), or custom TOML (`[[agents]]` / `[agent]` with `name`, `kind`, `executable`/`path`, `command`) via `--config`. Reuses local SSO/OAuth credentials with zero API key setup and supports `/model`, `/mode`, `/thinking` (`/effort`), `/commands`, and strict `y`/`n`/`Esc` permission approvals.
- **Standalone & Client-Daemon Execution:** Run single-process (`grein` / `grein standalone`) or split Client-Daemon (`grein daemon [--socket <SOCKET>]` + `grein attach [--socket <SOCKET>]`) with SQLite DAG storage in `.grein/session.db`.
- **Global `UserPrefsStore`:** Persists `theme`, `keybinding`, and `ui` (`sidebar_mode`, `sidebar_min_pct`, `sidebar_max_pct`, `sidebar_fixed_cols`, `sidebar_fixed_pct`, `sidebar_overflow`, `composer_autocomplete`) in `~/.config/grein/grein.db` (overridable via `$GREIN_CONFIG_DB` or `$XDG_CONFIG_HOME/grein/grein.db`).
- **Headless Inspection & Portable Config CLI:**
  - `grein doctor [--json]` checks `git`, `$SHELL`, ACP agents, `TERM`/`COLORTERM`/UTF-8 locale, `.grein/session.db`/socket health, and `grein.db`.
  - `grein status`, `grein threads`, and `grein agents` support `--json` for scripting.
  - `grein settings export|import` and `grein theme export|import` support `[FILE]` / `-` (stdout/stdin), `--format <toml|json|auto>`, `--only <sections>`, and merge-patch / `--replace` updates.
