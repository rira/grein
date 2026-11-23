# grein

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

**grein** is a high-performance, agent-agnostic Terminal User Interface (TUI) manager built around **Directed Acyclic Graph (DAG) Context Trees**. It prevents context pollution when exploring parallel sub-topics with AI coding agents while sharing foundational ancestor context along explicit branch lineage.

---

## Architectural Pillars

1. **Pure-Tree DAG Contexts:** Conversations form a Git-like DAG forest of immutable turn nodes organized into independent top-level roots (`ᛉ`) and explicit `parent_thread_id` child branches (`├─` / `└─`), alongside embedded interactive PTY terminal nodes (`>_`). Agent branches fork off structural anchors or active turns, inheriting ancestor turns up to the fork point while isolating sibling branches and separate roots. Terminal contexts inherit their parent's `cwd` (including opt-in `/worktree` directories). Manual `/join` / `/merge` (`m` in tree) commits a concise `DecisionLandmark` directly onto the parent branch (with append-only `[Joined Context]` delivery on active parent sessions, preserving LLM KV caches).
2. **Agent-Agnostic with ACP Protocol:** Interfaces directly with any installed CLI agent (e.g. `agy acp --stdio`, `claude --acp`) over standard JSON-RPC. Reuses existing SSO and local credentials with **zero API key requirement**.
3. **Workspace Isolation:** Opt-in `w` (in `ᛇ Grein`) or `/worktree` allocates an isolated `git worktree` `cwd` (with shadow snapshot fallback outside Git repositories) on either Agent or `>_` Terminal threads, inherited automatically by child terminals (`t` / `/term`).
4. **Terminal-First 2-Column Responsive TUI (Ratatui):** `grein` starts directly in a live `>_` `$SHELL` PTY terminal (`grein --agent` starts in ACP Agent mode). Left **`ᛇ Grein`** sidebar (`34%`, `ᛉ` Agent root markers, `>_` terminal nodes with `[exit N]` badges, Unicode `├─` / `└─` connectors, collapsible subtrees with `▸` / `(+N)` counts, `[joined]` badges) + right-hand live VT100 **`Terminal`** or **`Transcript`** (`66%` or `100%` borderless-sides zoom via `Ctrl+T`/`z`/`/zoom`, live preview when browsing the tree — hiding `Composer` during Terminal preview, collapsible tool/shell cards via `Ctrl+O`, vim `/` & `?` smart-case transcript search, syntax-highlighted code fences, GFM tables, truecolor/wide-glyph PTY cells, scrollback history, and `⎇ branch*` worktree git status badge). Press `a` in `ᛇ Grein` (or launch `grein --agent`) to create an Agent root with soft-wrapping auto-growing **`Composer`**; `Alt+T` jumps to or spawns an interactive `>_` terminal using `$SHELL`; Terminal threads hide `Composer`, pass all keys & bracketed paste directly to the PTY, support `Shift+PgUp/PgDn` & mouse wheel scrollback, restart on `Enter` once exited, and use `Ctrl+G` (or `Esc` once exited) to escape back to `ᛇ Grein`.

---

## Compact Keymap & Commands

| Scope | Binding / Command | Action |
| :--- | :--- | :--- |
| **Launch Modes** | `grein` / `grein --agent` | Default launches terminal-first in live `>_` `$SHELL`; `--agent` launches in ACP Agent mode |
| **Focus Toggle** | `Esc` / `Ctrl+G` | Toggle focus between `Composer`/`Terminal` and `ᛇ Grein` (auto-unhides sidebar when zoomed; in `ᛇ Grein`, activates the selected node; `Esc` cancels active agent turns, clears active search, or exits a dead PTY; `Ctrl+G` escapes live PTY passthrough without sending `\x07`) |
| **Terminal Jump** | `Alt+T` | Open or jump straight to the active/selected context's `>_` terminal (dynamically labeled with `$SHELL` basename; passes through cleanly inside `Terminal`) |
| **Branch Cycling** | `Tab` / `Shift+Tab` | Cycle branches within the active root's subtree (auto-unfolds hidden ancestors; inside `Terminal`, `Tab` forwards to PTY; in `ᛇ Grein` on a terminal node, `Tab`/`l`/`→`/`Enter` enters Terminal) |
| **Root Switching** | `Alt+Shift+↑` / `Alt+Shift+↓`, `Alt+J` / `Alt+K` | Switch between top-level roots |
| **Transcript & Search** | `Alt+↑`/`↓`, `PgUp`/`PgDn`, `Alt+Home`/`End`, `/` / `?` / `Ctrl+F` (`n`/`N`/`Esc`), `Ctrl+O`, `Ctrl+T`, `Alt+Y` | Scroll `Transcript`, vim-style forward/reverse smart-case search (`Ctrl+F` globally outside `Terminal`), toggle tool/shell cards, borderless zoom (resizes PTY), copy reply or terminal screen |
| **Context Lifecycle** | `Ctrl+N`, `Alt+N`, `Alt+R` / `F2` | Fork child branch / create blank root / rename active or selected context |
| **`ᛇ Grein` Sidebar** | `↑`/`↓`/`j`/`k`, `Space`/`h`/`←`/`l`/`→`, `Enter`, `J`/`K`, `Ctrl+U`/`D`, `g`/`G`, `t`/`T`, `a`/`n`, `w`, `f`, `r`, `m`, `d`/`x`, `z`, `y`, `q` | Browse (live preview) / fold-unfold subtrees (`Space`, `h`/`←` fold or jump parent, `l`/`→` unfold or activate) / activate / prev/next root / scroll / top/bottom / child or root terminal / new Agent root / allocate worktree (Agent or `>_`) / fork / rename / join / close / zoom / copy / quit |
| **Slash & Shell** | `/term [path]` (`/terminal`, `/zsh`, `/sh`, `/shell`, `/bash`), `/worktree`, `/fork`, `/root`, `/new`, `/rename`, `/join`, `/merge`, `/close`, `/model`, `/mode`, `/thinking`, `/commands`, `/copy [code\|all]`, `/zoom`, `/help` (`/?`), `!cmd` | Open PTY terminal (inherits parent `cwd` or uses `path`, case-insensitive) / opt-in thread git worktree on Agent or `>_` Terminal (`⎇ branch*` badge) / branch / blank root / rename / join (`DecisionLandmark`) / close / ACP config picker & args / agent commands / clipboard / zoom / help / PTY shell |
| **Terminal (`>_`)** | `Ctrl+G`, `Shift+PgUp`/`PgDn` / `Alt+PgUp`/`PgDn` / wheel, `Enter` / `Esc` (exited), `Ctrl+T`, `Ctrl+Q`, all other keys & paste | Return focus to `ᛇ Grein` / scroll PTY history (`[scroll -N]`) / `Enter` restarts exited PTY (`Esc` escapes to tree) / toggle zoom & resize PTY / quit; all other keys (`Ctrl+C`, live `Esc`, `Alt+T`, `Tab`, `F1`, `Ctrl+F`, arrows, UTF-8) & bracketed paste pass straight to PTY |
| **Composer & Help** | `Shift+Enter` / `Ctrl+J`, `Ctrl+A`/`E`, `Ctrl+B`/`←`/`→`, `Alt+←`/`→`, `Ctrl+K`/`U`/`W`/`Y`/`D`, `F1` / `g?` / empty `?` / `/help`, `Ctrl+C`/`Q` | Newline, Readline editing & kill ring, scrollable help modal (`F1` outside `Terminal`, `g?` in `ᛇ Grein`/`Transcript`, `?` in empty `Composer`), quit |

> **Terminal Tip (macOS / tmux):** In Terminal.app or iTerm2, enable *Use Option as Meta key* so `Alt+` shortcuts work natively, or use zero-modifier `ᛇ Grein` keys (`J`/`K`, `g`/`G`, `Ctrl+U`/`D`, `z`, `y`, `f`). Inside `tmux`, `grein` uses `Ctrl+B` solely for Composer `backward-char` and forwards OSC 52 clipboard passthrough automatically.

---

## Workspace Structure

* `crates/grein-core`: Message DAG graph model, SQLite persistence (`rusqlite`), and heuristic anchor parser.
* `crates/grein-acp`: Agent Client Protocol (ACP) JSON-RPC stdio subprocess driver and permission policy gate.
* `crates/grein-workspace`: Git worktree allocator, shadow snapshot fallback engine, and PTY shell runner.
* `crates/grein-tui`: Pure 2-column Ratatui terminal interface, Markdown/GFM/ANSI renderer, and non-blocking event multiplexer.
* `crates/grein-cli`: The unified executable (`grein`) supporting standalone, daemon/attach, and headless inspection modes.

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
