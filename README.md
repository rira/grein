# grein

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

**grein** is a high-performance, agent-agnostic Terminal User Interface (TUI) manager built around **Directed Acyclic Graph (DAG) Context Trees**. It prevents context pollution when exploring parallel sub-topics with AI coding agents while sharing foundational ancestor context along explicit branch lineage.

---

## Architectural Pillars

1. **Pure-Tree DAG Contexts:** Conversations form a Git-like DAG forest of immutable turn nodes organized into independent top-level roots (`ᛉ`) and explicit `parent_thread_id` child branches (`├─` / `└─`). Branches fork off structural anchors or active turns, inheriting ancestor turns up to the fork point while isolating sibling branches and separate roots. Manual `/join` / `/merge` (`m` in tree) commits a concise `DecisionLandmark` directly onto the parent branch (with append-only `[Joined Context]` delivery on active parent sessions, preserving LLM KV caches).
2. **Agent-Agnostic with ACP Protocol:** Interfaces directly with any installed CLI agent (e.g. `agy acp --stdio`, `claude --acp`) over standard JSON-RPC. Reuses existing SSO and local credentials with **zero API key requirement**.
3. **Workspace Isolation:** Dynamically uses isolated `git worktree`s for code-modifying threads while reads operate directly against the shared working directory. Gracefully falls back to directory shadow snapshots outside Git repositories.
4. **Pure 2-Column Responsive TUI (Ratatui):** Left **`ᛇ Grein`** sidebar (`34%`, `ᛉ` root markers, Unicode `├─` / `└─` connectors, `[joined]` badges) + full-width **`Transcript`** (`66%` or `100%` borderless-sides copy zoom via `Ctrl+T`/`z`/`/zoom`, live preview when browsing the tree, collapsible tool/shell cards via `Ctrl+O`, syntax-highlighted code fences, GFM tables, ANSI PTY shell output) + soft-wrapping auto-growing **`Composer`** (with Readline bindings and `!cmd` shell execution) + interactive `/model`/`/mode`/`/thinking` config picker + auto-hiding **`Subagents`** strip and 1-line status bar. No Blackboard side pane or shared working memory injection.

---

## Compact Keymap & Commands

| Scope | Binding / Command | Action |
| :--- | :--- | :--- |
| **Focus Toggle** | `Esc` / `Ctrl+G` | Toggle focus between `Composer` and `ᛇ Grein` (`Esc` cancels active streaming turns; `Ctrl+G` toggles without cancelling) |
| **Branch Cycling** | `Tab` / `Shift+Tab` | Cycle branches within the active root's subtree (no-op on lone roots) |
| **Root Switching** | `Alt+Shift+↑` / `Alt+Shift+↓`, `Alt+J` / `Alt+K` | Switch between top-level roots |
| **Transcript & Zoom** | `Alt+↑`/`↓`, `PgUp`/`PgDn`, `Alt+Home`/`End`, `Ctrl+O`, `Ctrl+T`, `Alt+Y` | Scroll `Transcript`, toggle collapsible tool/shell cards, toggle borderless-sides copy zoom, copy last reply |
| **Context Lifecycle** | `Ctrl+N`, `Alt+N`, `Alt+R` / `F2` | Fork child branch / create blank root / rename active or selected context |
| **`ᛇ Grein` Sidebar** | `↑`/`↓`/`j`/`k`, `Enter`, `J`/`K`, `Ctrl+U`/`D`, `g`/`G`, `f`, `n`, `r`, `m`, `d`/`x`, `z`, `y`, `q` | Browse (live preview) / activate / prev/next root / half-page scroll / top/bottom / fork / new root / rename / join / close / copy zoom / copy last reply / quit |
| **Slash & Shell** | `/fork`, `/root`, `/new`, `/rename`, `/join`, `/merge`, `/close`, `/model`, `/mode`, `/thinking`, `/commands`, `/copy [code\|all]`, `/zoom`, `!cmd` | Branch / blank root / rename / join (`DecisionLandmark`) / close / ACP config picker & args / agent commands / clipboard / zoom / PTY shell |
| **Composer & App** | `Shift+Enter` / `Ctrl+J`, `Ctrl+A`/`E`, `Ctrl+F`/`B`, `Alt+←`/`→`, `Ctrl+K`/`U`/`W`/`Y`/`D`, `?`, `Ctrl+C`/`Q` | Newline, Readline editing & kill ring, scrollable help modal (`?`), quit |

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
