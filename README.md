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
   - Composer `/worktree ` (`/wt `) triggers live path/branch autocomplete prioritizing the active repo (`↑`/`↓` navigate the popup, `Tab` completes, `Enter` allocates or attaches).
   - Pressing `Tab` in `MANAGE` mode switches to the per-repo sorted **`⎇ Worktrees`** sidebar (active repo first, `(primary)` pinned first, `*N` dirty & `[N]` attached badges) paired with a live colorized **`⎇ Diff`** preview (`git status --short --branch`, `--stat`, and `git diff HEAD`) in the right pane and an `⎇ worktree: <name>` bottom-right badge.
4. **Strict 2-Mode Terminal-First TUI, Unified Naming & Full-Height Layout:**
   - **Strict WRITE vs. MANAGE Modes:** Starts directly in **WRITE** mode inside a live `>_` `$SHELL` PTY (`grein --agent` starts in an Agent `Composer`). In `WRITE` mode, identical intercepts apply across both Agent `Composer` and Embedded `>_` Terminal: `Ctrl+G` (single canonical gateway toggling `WRITE <-> MANAGE`, customizable via `:bind enter_manage_mode ...`), `Alt+Z` (hide/show sidebar toggle), `Alt+K` / `Alt+J` (Vim-first scroll up/down in transcript or shell scrollback), `Alt+H` / `Alt+L` (switch prev/next sibling thread within the active `parent_thread_id` level), and `Alt+1..9` (jump directly to root threads `1..9`, `alt && !ctrl`, Swedish `AltGr`-safe). Both `Alt+K`/`Alt+J` and `Alt+H`/`Alt+L` pass through untouched in `>_` Terminal whenever an alternate-screen app (`nvim`, `less`, `htop`, etc.) is active, while Swedish/international `AltGr` characters (`@£${}[]\~|€`), `Esc`, `Tab`, `Ctrl+T`, `Ctrl+Q`, `Ctrl+F`, and `Ctrl+N` stay 100% collision-free. Mouse capture stays disabled so native terminal text selection always works.
   - **Visual Polish & Live-Preview Themes:** Rounded pane borders (`╭─╮╰─╯`), `▌` flat row selection (no `REVERSED`), themed mode pills (`WRITE · SHELL`, `WRITE · AGENT`, `MANAGE · GREIN`, `MANAGE · WORKTREES`), `⎇ worktree: <name>` bottom-right badge, and 4 built-in themes (`tokyo-night`, `catppuccin-mocha`, `nord`, `ansi`) with `:theme` / `/theme` live-preview modal and hex/ANSI color overrides persisted to global SQLite preferences (`~/.config/grein/grein.db` or `$GREIN_CONFIG_DB`).
   - **Unified `[given name] auto-context` Naming & Full-Height Sidebar:**
     - Root rows display a dim `1..9` index prefix (`1 ᛉ Main`, `2 ᛉ [Auth] Test Conversation`).
     - Terminals (`>_`) show live `cwd` (`>_ ~/projects` when unnamed, `>_ [Test] ~/projects` when renamed; explicit `"Terminal"` is preserved as `[Terminal] ~/...`, an empty rename clears `[given name]` back to auto-only, and isolated worktrees format cleanly as `worktree: <branch-or-8-char>` instead of raw UUIDs). Live directories sync via OSC 7 plus OS process `cwd` polling fallback (`/proc/<pid>/cwd` on Linux, `lsof` on macOS), and child PTYs receive `GREIN=1`.
     - Agents (`ᛉ`) show their ACP session auto-title (`Test Conversation`) or `[Auth] Test Conversation` when named.
     - The sidebar extends full-height to the status bar in all views (both Agent and Terminal) with identical width across `ᛇ Grein` and `⎇ Worktrees`, a `{n} agent · {m} shell` footer, and `:sidebar` layout sizing (`dynamic [min%] [max%]` default `18%..32%`, `fixed <cols|pct%>`, `overflow on|off`).

---

## Compact Keymap & Commands

Launch modes: `grein` opens terminal-first in `$SHELL`; `grein --agent` starts an ACP agent session.

| Mode / Scope | Binding / Command | Action |
| :--- | :--- | :--- |
| **WRITE Mode (Global)** | `Ctrl+G`, `Alt+Z`, `Alt+K`/`Alt+J`, `Alt+H`/`Alt+L`, `Alt+1..9` | Identical intercepts in `Composer` & `>_` Terminal: `Ctrl+G` toggles `WRITE <-> MANAGE` (auto-unhides sidebar; customizable via `:bind enter_manage_mode ...`); `Alt+Z` hides/shows sidebar (`sidebar` toggle); `Alt+K`/`Alt+J` Vim-first scroll up/down in transcript or shell scrollback; `Alt+H`/`Alt+L` switch prev/next sibling within active `parent_thread_id` level (`Alt+K`/`J`/`H`/`L` pass through untouched in alternate-screen PTY apps like `nvim`, `less`, `htop`); `Alt+1..9` jump to roots `1..9` (`alt && !ctrl`, Swedish `AltGr`-safe) |
| **WRITE Mode (Shell `>_`)** | All other keys & bracketed paste; `Enter` / `Esc` (when exited) | Forward directly to `$SHELL` (`Ctrl+T`, `Ctrl+Q`, `Ctrl+F`, `Ctrl+N`, live `Esc`, `Tab`, `F1`, `?`, UTF-8, Swedish `AltGr`; plus `Alt+K`/`J`/`H`/`L` pass-through in alternate-screen apps); native terminal selection preserved (mouse capture disabled); when exited, `Enter` restarts PTY and `Esc` enters `MANAGE` |
| **WRITE Mode (Composer)** | `Enter`, `Shift+Enter` / `Ctrl+J`, `↑` / `↓`, Readline (`Ctrl+A/E/B/F/K/U/W/Y/D`, `Alt+←/→/B/F/D`), `Ctrl+C`, `Esc`, `/wt ` + `↑`/`↓`/`Tab` | Submit prompt / `/slash` / `!cmd`; insert newline; multi-line `↑`/`↓` navigate across lines preserving horizontal column (`↑`/`↓` navigate `/worktree` popup when open, `Tab` completes); edit with kill ring; zero global TUI collisions on `Esc`, `Tab`, `Ctrl+T`, `Ctrl+Q`, `Ctrl+F`, `Ctrl+N`, or `AltGr` chars; `Ctrl+C` cancels busy turn → clears draft → shows exit hint (never quits); `Esc` closes autocomplete or cancels turn |
| **MANAGE Mode (Shared)** | `Tab`, `?`, `z`, `:`, `/`, `q` / `Ctrl+C`, `Ctrl+U`/`Ctrl+D`, `Ctrl+F`/`Ctrl+B` | `Tab` switches `ᛇ Grein` ↔ `⎇ Worktrees`; `?` toggles scrollable Help modal; `z` hides sidebar; `:` opens Sidebar Command Palette; `/` filters (`⎇ Worktrees`) or searches (`ᛇ Grein`); `q` / `Ctrl+C` quits (confirmed if shells live or agents busy); `Ctrl+U`/`Ctrl+D` Vim half-page and `Ctrl+F`/`Ctrl+B` Vim full-page right-pane preview scroll |
| **MANAGE (Navigation & Folding)** | `j`/`↓`, `k`/`↑`, `gg`/`G`, `h`/`←`, `l`/`→`, `Enter`, `H`/`L`, `J`/`K`, `Alt+1..9`, `Esc` | Browse with live right-pane preview; top/bottom; `←`/`h` collapse or jump to parent; `→`/`l` expand or step into first child (**stays in `MANAGE`** — never exits to `WRITE`); `Enter` (or `Esc` when no `/` search/filter is active) switches back to `WRITE` (or attaches worktree in `⎇ Worktrees`); `H`/`L` collapse all / expand all branches (or repos); `J`/`K` switch prev/next root; `Alt+1..9` jump to roots `1..9`; `Esc` clears active `/` search/filter |
| **MANAGE (`ᛇ Grein` Actions)** | `n` (`n`/`N`), `f`, `s` / `S`, `w`, `r`, `m`, `d`, `o`, `y` | 1-to-1 mnemonics: `[n]ew` root agent (`n`/`N` step search matches while `/` highlights are active); `[f]ork` branch; `[s]hell` child / `S` root `>_` shell; `[w]orktree`; `[r]ename` (`[given name]` inline; empty clears back to auto-context); `[m]erge` (`DecisionLandmark`); `[d]el` (confirmed); `o` toggle tool/shell cards; `y` copy reply or screen (`pbcopy`/`wl-copy`/`xclip`/`xsel` + OSC 52) |
| **MANAGE (`⎇ Worktrees` Actions)** | `Enter`, `s`, `w`, `p`, `r`, `d`, `y` | 1-to-1 mnemonics: `[↵]attach` worktree & enter `WRITE`; `[s]hell` here; `[w]orktree` allocate; `[p]rune` stale worktrees; `[r]efresh`; `[d]el` (confirmed); `y` copy path |
| **Palette & Slash Commands** | `/theme [name]`, `/sidebar [...]`, `/bind`, `/unbind`, `/keybinds` (`/binds`), `/zoom`, `/help` (`/?`), `/quit` (`/q`, `/exit`) | Live theme picker or preset (`tokyo-night`, `catppuccin-mocha`, `nord`, `ansi`); sidebar `dynamic [min max]` / `fixed <cols\|pct%>` / `min` / `max` / `overflow <on\|off>`; customize `WRITE` keybinds; toggle sidebar visibility; help; quit (prefix `/` in `Composer` or `:` in `MANAGE` Palette) |
| **Context, Agent & Worktree Slash** | `/root` (`/new`), `/fork` (`/branch`, `/grein`), `/rename`, `/join` (`/merge`), `/delete` (`/close`), `/term [path]` (`/terminal`, `/zsh`, `/sh`, `/shell`, `/bash`), `/worktree [path\|query]` (`/wt`), `/worktrees [filter]`, `/prune`, `/model`, `/mode`, `/thinking` (`/effort`), `/commands`, `/copy [code\|all]`, `!cmd` | Create root / fork / rename / join / delete context; open shell; allocate/attach worktree (with `Composer` autocomplete); open `⎇ Worktrees`; prune stale worktrees; pick or set ACP model/mode/effort (with `--default`); list agent commands; copy reply/code/transcript; run inline shell command |
| **Modals** | `y` / `n` / `Esc` (Permission, Delete, Quit); `j`/`k`/`Tab`/`Enter`/`Esc`/`q` (Theme/Config/Help) | Modals intercept keys with top priority; `Permission Required` strictly requires explicit `y`/`n`/`Esc` (`Enter` does not approve) |

> **Terminal Tip (macOS / tmux):** In Terminal.app or iTerm2, enable *Use Option as Meta key* so `Alt+` shortcuts (`Alt+Z`, `Alt+K`/`J`, `Alt+H`/`L`, `Alt+1..9`) work natively, or use `Ctrl+G` to enter `MANAGE` mode. Inside `tmux`, `grein` uses `Ctrl+B` solely for `Composer` `backward-char` and `MANAGE`/Help full-page scroll up (never intercepted in `WRITE · SHELL`), scrubs inherited multiplexer variables from embedded PTYs, and wraps OSC 52 clipboard passthrough automatically.

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

The reproducible [VHS](https://github.com/charmbracelet/vhs) script in [`docs/demo.tape`](./docs/demo.tape) demonstrates terminal-first `grein`, Vim-first `Alt+K`/`Alt+J` shell scrollback, `Ctrl+G` into `MANAGE`, creating a `[n]ew` root agent and child `[s]hell` threads, `Alt+H`/`Alt+L` sibling switching, confirmed `[d]el`, switching to `⎇ Worktrees` via `Tab` to inspect a worktree diff preview, opening the `:` command palette for live `:theme` switching, and pressing `Enter` to re-enter `$SHELL` `WRITE` mode.

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
