# grein

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

**grein** is a high-performance, agent-agnostic Terminal User Interface (TUI) manager built around **Directed Acyclic Graph (DAG) Context Trees**. It prevents context pollution when exploring parallel sub-topics with AI coding agents while sharing foundational ancestor context along explicit branch lineage.

---

## Installation

### Homebrew (macOS & Linux)

```bash
# Primary tap
brew install rira/tap/grein

# Source fallback directly from this repository
brew tap rira/grein https://github.com/rira/grein && brew install rira/grein/grein
```

### Nix Flake

```bash
# Run directly without installing
nix run github:rira/grein

# Or install into your Nix profile
nix profile install github:rira/grein
```

### From Source (Cargo)

```bash
cargo install --path crates/grein-cli --locked
```

### Launch Modes

- `grein` opens terminal-first in `$SHELL`.
- `grein --agent` starts an ACP agent session.

---

## Architectural Pillars

1. **Pure-Tree DAG Contexts:** Conversations form a Git-like DAG forest of immutable turn nodes organized into independent top-level roots (`ᛉ`) and explicit `parent_thread_id` child branches (`├─` / `└─`), alongside embedded interactive PTY terminal nodes (`>_`). Agent branches fork off structural anchors or active turns, inheriting ancestor turns up to the fork point while isolating sibling branches and separate roots. Terminal contexts inherit their parent's `cwd` (including `/worktree` directories). Manual `/join` / `/merge` (`m` in `ᛇ Grein`) commits a concise `DecisionLandmark` directly onto the parent branch (with append-only `[Joined Context]` delivery on active parent sessions, preserving LLM KV caches).
2. **Agent-Agnostic with ACP Protocol:** Interfaces directly with any installed CLI agent (e.g. `agy`, `claude`, or configured custom ACP executables) over standard JSON-RPC. Reuses existing SSO and local credentials with **zero API key requirement**, supporting runtime `/model`, `/mode`, `/thinking` (`/effort`), and `/commands` discovery.
3. **Branch-Based Worktree Isolation & `⎇ Worktrees` View:** Opt-in `w` or `/worktree` (`/wt`) allocates an isolated `git worktree` (with shadow snapshot fallback outside Git repositories) using **Option 1 branch-based naming**:
   - On non-default branches (anything other than `main`/`master` or detached `HEAD`), creates `<primary_repo_root>/.grein/worktrees/<branch-slug>` on branch `<branch>-wt` (auto-suffixing `-2`, `-3`, … on collisions and always anchoring at the primary repo root so linked worktrees never nest `.grein/worktrees/`). Falls back to `.grein/worktrees/<thread_id>` (`worktree-<thread_id>`) on `main`, `master`, or detached `HEAD`, and keeps `.grein/` in git `info/exclude`.
   - Composer `/worktree ` (`/wt `) triggers live path/branch autocomplete prioritizing the active repo (`Tab` completes, `Enter` allocates or attaches).
   - Pressing `Tab` or `Ctrl+T` in MANAGE mode switches to the per-repo sorted **`⎇ Worktrees`** sidebar (active repo first, `(primary)` pinned first, `*N` dirty & `[N]` attached badges) paired with a live colorized **`⎇ Diff`** preview (`git status --short --branch`, `--stat`, and `git diff HEAD`) in the right pane.
4. **Strict 2-Mode Terminal-First TUI, Themes & Smart Layout:**
   - **Strict WRITE vs. MANAGE Modes:** Starts directly in **WRITE** mode inside a live `>_` `$SHELL` PTY (`grein --agent` starts in an Agent `Composer`). In WRITE mode, only configurable `Keymap` intercepts (`Ctrl+G` / `Alt+G` / `F6` for MANAGE, `Alt+Z` for zoom, `Alt/Shift+PgUp/PgDn` for scroll) are captured — leaving `Ctrl+T`, `Ctrl+Q`, `Ctrl+F`, `Ctrl+N`, `Esc`, `Tab`, `F1`, `?`, `Alt+1..9`, and Swedish/international `AltGr` characters (`@£${}[]\~|€`) 100% collision-free inside `$SHELL` and `Composer`.
   - **Visual Polish & Live-Preview Themes:** Rounded pane borders (`╭─╮╰─╯`), `▌` flat row selection (no `REVERSED`), themed mode pills (`WRITE · SHELL`, `WRITE · AGENT`, `MANAGE · GREIN`, `MANAGE · WORKTREES`), and 4 built-in themes (`tokyo-night`, `catppuccin-mocha`, `nord`, `ansi`) with `:theme` / `/theme` live-preview modal and hex/ANSI color overrides persisted to global SQLite preferences (`~/.config/grein/grein.db` or `$GREIN_CONFIG_DB`).
   - **Smart PTY Labels, Live `cwd` Sync & Configurable Sidebar:** Terminal `>_` nodes show live folder basenames (shortening worktrees to `⎇ <slug>`), track live directory changes via OSC 7 plus OS process `cwd` polling fallback (`/proc/<pid>/cwd` on Linux, `lsof` on macOS), and export `GREIN=1` into child PTYs. `:sidebar` configures `dynamic [min%] [max%]` (default `18%..32%`), `fixed <cols|pct%>`, and `overflow on|off` (1-row active-row hover overlay across the divider in MANAGE mode without resizing PTYs).

---

## Compact Keymap & Commands

Launch modes: `grein` opens terminal-first in `$SHELL`; `grein --agent` starts an ACP agent session.

| Mode / Scope | Binding / Command | Action |
| :--- | :--- | :--- |
| **WRITE Mode (Global)** | `Ctrl+G` / `Alt+G` / `F6` | Toggle `WRITE <-> MANAGE` sidebar (auto-unhides sidebar if zoomed and resizes PTY; `enter_manage_mode`) |
| **WRITE Mode (Layout & Scroll)** | `Alt+Z`, `Alt+PgUp`/`PgDn`, `Shift+PgUp`/`PgDn`, Mouse Wheel | Toggle borderless full-width sidebar zoom (`toggle_zoom`) / scroll active PTY (`[scroll -N]`) or `Transcript` |
| **WRITE Mode (Shell `>_`)** | All other keys & bracketed paste; `Enter` / `Esc` (when exited) | Forward directly to `$SHELL` (`Ctrl+T`, `Ctrl+Q`, `Ctrl+F`, `Ctrl+N`, live `Esc`, `Tab`, `F1`, `?`, `Alt+1..9`, UTF-8, `AltGr`); when exited, `Enter` restarts PTY and `Esc` enters MANAGE |
| **WRITE Mode (Composer)** | `Enter`, `Shift+Enter` / `Ctrl+J`, Readline (`Ctrl+A/E/B/F/K/U/W/Y/D`, `Alt+←/→/B/F/D`), `Ctrl+C`, `Esc`, `/wt ` + `Tab` | Submit prompt / `/slash` / `!cmd`; insert newline; edit with kill ring; `AltGr` chars preserved; `Ctrl+C` cancels busy turn → clears draft → shows exit hint (never quits); `Esc` closes autocomplete or cancels turn |
| **MANAGE Mode (Shared)** | `Tab` / `Shift+Tab` / `Ctrl+T`, `:` , `?` / `F1` (`g?`), `z`, `q` / `Ctrl+C`, `PgUp`/`PgDn`, `Ctrl+U`/`Ctrl+D` | Switch `ᛇ Grein` ↔ `⎇ Worktrees`; open Command Palette (`:`); toggle scrollable Help modal; toggle zoom; confirmed quit (`Confirm Quit` modal if shells live or agents busy); scroll right-pane preview (diff / chat / `>_`) |
| **MANAGE (`ᛇ Grein`)** | `j`/`↓`, `k`/`↑`, `gg`/`G`, `J`/`K`, `Alt+J`/`K`, `Alt+Shift+↑`/`↓`, `Space`/`h`/`←`/`l`/`→`, `Enter`, `Esc` | Browse with live right-pane preview; top/bottom; `J`/`K` next/prev root (`Alt+J`/`K` & `Alt+Shift+↑`/`↓` also switch roots); fold/unfold (`Space`, `h`/`←` fold/parent, `l`/`→` unfold/activate); `Enter` activates & enters WRITE; `Esc` clears search or enters WRITE |
| **MANAGE (`ᛇ Grein` Actions)** | `a`, `f`, `t` / `T` (`Alt+T`), `w`, `m`, `r`, `d`/`x`, `o`, `y`, `/` (`n`/`N`) | New root Agent; fork branch; jump/new child `>_` shell (`T` root `>_`); allocate worktree; join branch (`DecisionLandmark`); inline rename; confirmed delete; toggle tool/shell cards; copy reply or screen (`pbcopy`/`wl-copy`/`xclip`/`xsel` + OSC 52); smart-case search |
| **MANAGE (`⎇ Worktrees`)** | `j`/`k`/`gg`/`G`, `Space`/`h`/`l`, `Enter`, `t`, `w`/`a`/`n`, `r`, `p`, `y`, `d`/`x`, `/` | Browse per-repo grouped worktrees with live `⎇ Diff` preview; fold/unfold repo; `Enter` folds repo or attaches worktree & enters WRITE; `t` spawns shell here; `w`/`a`/`n` allocates; `r` refreshes; `p` prunes stale worktrees; `y` copies path; `d`/`x` confirmed delete; `/` filters |
| **Palette & Slash Commands** | `/theme [name]`, `/sidebar [...]`, `/bind`, `/unbind`, `/keybinds` (`/binds`), `/zoom`, `/help` (`/?`), `/quit` (`/q`, `/exit`) | Live theme picker or preset (`tokyo-night`, `catppuccin-mocha`, `nord`, `ansi`); sidebar `dynamic [min max]` / `fixed <cols\|pct%>` / `min` / `max` / `overflow <on\|off>`; customize WRITE keybinds; zoom; help; quit (prefix `/` in Composer or `:` in MANAGE Palette) |
| **Context, Agent & Worktree Slash** | `/root` (`/new`), `/fork` (`/branch`, `/grein`), `/rename`, `/join` (`/merge`), `/delete` (`/close`), `/term [path]` (`/terminal`, `/zsh`, `/sh`, `/shell`, `/bash`), `/worktree [path\|query]` (`/wt`), `/worktrees [filter]`, `/prune`, `/model`, `/mode`, `/thinking` (`/effort`), `/commands`, `/copy [code\|all]`, `!cmd` | Create root / fork / rename / join / delete context; open shell; allocate/attach worktree (with Composer autocomplete); open `⎇ Worktrees`; prune stale worktrees; pick or set ACP model/mode/effort (with `--default`); list agent commands; copy reply/code/transcript; run inline shell command |
| **Modals** | `y` / `n` / `Esc` (Permission, Delete, Quit); `j`/`k`/`Tab`/`Enter`/`Esc`/`q` (Theme/Config/Help) | Modals intercept keys with top priority; `Permission Required` strictly requires explicit `y`/`n`/`Esc` (`Enter` does not approve) |

> **Terminal Tip (macOS / tmux):** In Terminal.app or iTerm2, enable *Use Option as Meta key* so `Alt+` shortcuts work natively, or use `Ctrl+G` / `F6` to enter MANAGE mode. Inside `tmux`, `grein` uses `Ctrl+B` solely for Composer `backward-char`, scrubs inherited multiplexer variables from embedded PTYs, and wraps OSC 52 clipboard passthrough automatically.

---

## CLI Tools & Headless Commands

Alongside the interactive TUI (`grein`, `grein --agent`, `grein daemon`, `grein attach`), `grein` provides headless inspection, diagnostics, and portable preference management:

* **Diagnostics (`grein doctor`):**
  ```bash
  grein doctor [--json]
  ```
  Inspects workspace Git state, discovered ACP agent executables (`agy`, `claude`, custom config), session database/socket paths, and global preferences store health.
* **Headless State Inspection:**
  ```bash
  grein status [--json] [--socket <SOCKET>] [--db <DB>]
  grein threads [--json] [--db <DB>]
  grein agents [--json] [--config <CONFIG>]
  ```
* **Settings Export & Merge-Patch Import (`grein settings`):**
  Exports and imports global preferences (`~/.config/grein/grein.db`, overridable via `$GREIN_CONFIG_DB` or `$XDG_CONFIG_HOME/grein/grein.db`) in **TOML** and **JSON** across `theme`, `keybinding`, and `ui` sections:
  ```bash
  grein settings export [--only <sections>]
  grein settings import <file> [--only <sections>] [--replace]
  ```
  * **Merge-patch semantics:** Omitted sections and keys remain untouched. Setting a section to `null` (JSON `"keybinding": null`, TOML `keybinding = null`) or an empty TOML table (`[keybinding]`) resets that section to defaults; setting an individual key to `null` resets that key. Pass `--replace` to replace the target scope outright.
* **Shareable Themes (`grein theme`):**
  ```bash
  grein theme export
  grein theme import <file> [--replace]
  ```
  Exports and imports theme presets plus custom color overrides (`[theme]` or flat color keys) without modifying keybindings or UI layout settings.

---

## Workspace Structure

* [`crates/grein-core`](./crates/grein-core): Message DAG graph model, SQLite persistence (`rusqlite`), heuristic anchor parser, and global `UserPrefsStore` with TOML/JSON merge-patch settings/theme export & import.
* [`crates/grein-acp`](./crates/grein-acp): Agent Client Protocol (ACP) JSON-RPC stdio subprocess driver and permission policy gate.
* [`crates/grein-workspace`](./crates/grein-workspace): Branch-named Git worktree allocator, per-repo worktree discovery/prune/diff preview, shadow snapshot fallback engine, and VT100 PTY shell runner with OSC 7 + OS `cwd` sync and `GREIN=1`.
* [`crates/grein-tui`](./crates/grein-tui): Strict 2-mode Ratatui interface (`WRITE` / `MANAGE`), `ᛇ Grein` & `⎇ Worktrees` sidebars, rounded themed chrome, customizable keymap, command palette, Markdown/GFM/ANSI renderer, and non-blocking event multiplexer.
* [`crates/grein-cli`](./crates/grein-cli): The unified executable (`grein`) supporting standalone, daemon/attach, diagnostics, preferences CLI, and headless inspection modes.

---

## Demo Recording

The reproducible [VHS](https://github.com/charmbracelet/vhs) script in [`docs/demo.tape`](./docs/demo.tape) demonstrates terminal-first `grein`, switching to `⎇ Worktrees` via `Ctrl+G` + `Tab`, inspecting a worktree diff preview, opening the `:` command palette for live `:theme` switching, and returning to the shell.

---

## Building & Verification

```bash
# Check all workspace crates
cargo check

# Run test suite
cargo test

# Build release binary
cargo build --release
```

Detailed architectural blueprints, protocol definitions, and UI specifications are maintained in [`SPECIFICATION.md`](./SPECIFICATION.md).
