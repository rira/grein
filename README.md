# grein

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

**grein** is a terminal-first, agent-agnostic TUI manager built around **Directed Acyclic Graph (DAG) Context Trees**, branch-named Git worktrees, and embedded interactive PTY shells. It prevents context pollution when exploring parallel sub-tasks with AI coding agents while sharing foundational ancestor context along explicit branch lineage.

---

## Installation

```bash
# Homebrew (macOS & Linux)
brew install rira/tap/grein
# Source fallback from this repository:
brew tap rira/grein https://github.com/rira/grein && brew install rira/grein/grein

# Nix Flake (run directly or install into your profile)
nix run github:rira/grein
nix profile install github:rira/grein

# From Source (Cargo)
cargo install --path crates/grein-cli --locked
```

### Launch Modes

- `grein` opens terminal-first in `$SHELL` (`GREIN=1`).
- `grein --agent` starts an ACP agent session using your host-installed default provider.
- **Zero-bundling ACP (Claude Code & custom agents):** `grein` does not bundle external agent binaries. Anthropic's standalone `claude` CLI is **not** an ACP server—install the `claude-agent-acp` (or `claude-code-acp`) adapter binary on `$PATH`, or configure a custom `[[agents]]` entry (such as `command = ["npx", "-y", "@agentclientprotocol/claude-agent-acp"]`):
  ```bash
  # macOS / non-Nix (requires Node >= 22)
  npm install -g @agentclientprotocol/claude-agent-acp

  # Nix (ephemeral shell without global install)
  NIXPKGS_ALLOW_UNFREE=1 nix shell --impure nixpkgs#claude-agent-acp -c grein
  ```
- See [`docs/demo.tape`](./docs/demo.tape) for a reproducible [VHS](https://github.com/charmbracelet/vhs) walkthrough.

---

## Highlights

- **`ᛇ Grein` Pure-Tree DAG Sessions:** Independent roots (`1..9`), explicit `parent_thread_id` child forks, sibling isolation, subtree collapsing (`Space`/`h`/`l`/`H`/`L`), `[custom name] auto context` naming, `/join` `DecisionLandmark` synthesis (with KV-cache-friendly `[Joined Context]` delivery), and a live `Subagents` activity strip.
- **`⎇ Worktrees` & Non-Git Shadow Fallback:** Branch-based `<primary_repo_root>/.grein/worktrees/<branch-slug>` (`<branch>-wt`) isolation, `Target:`-synced `⎇ Worktrees` sidebar (`Tab`), live `⎇ Diff` preview, Composer `/wt` autocomplete, and copy-on-write `.grein/shadow/` snapshots outside Git.
- **Embedded `>_` PTY Terminals:** 24-bit color VT100 terminal in `$SHELL`, 1,000-line scrollback buffer, `OSC 7` + OS `cwd` sync, `DSR`/`DA1`/`DA2`/`OSC 10`/`11` responses, Swedish `AltGr` UTF-8 input, and automatic `alternate_screen` passthrough (`nvim`/`htop`/`tmux`).
- **Composer & Autocomplete:** Visual multiline navigation, full readline editing, and Tier 1/2/3 inline ghost-text autocomplete (`/` commands including `/ag` → `/agent`, `@files` & branches, prompt history + transcript tokens, and a 6,878-word dictionary).
- **16 Themes, Fuzzy Help & Copy/Paste:** Live-previewed `:theme` picker with 5-dot swatches and tone tags, `?` Help with `/` fuzzy filter, and pane-scoped mouse drag/double/triple-click selection (`pbcopy`/`wl-copy`/`xclip`/`xsel` + OSC 52).
- **Multi-Provider Zero-Bundling ACP:** Discovers host `agy`, `claude-agent-acp` → `claude-code-acp` (standalone `claude` CLI is not an ACP server), `$GREIN_ACP_BIN`, or custom `[[agents]]` configs with zero API key setup, runs concurrent per-provider sessions, and spawns siblings via `/agent`, `A`, or `:<provider>` alongside `/model`, `/mode`, `/thinking`, and `/commands`.

---

## Essential Keys

`grein` uses a strict 2-mode layout: **`WRITE`** (`>_` Shell or Agent `Composer`) and **`MANAGE`** (`ᛇ Grein` / `⎇ Worktrees` sidebar).

| Key | Scope | Action |
| :--- | :--- | :--- |
| `Ctrl+G` | Global (`WRITE` ↔ `MANAGE`) | Toggle sidebar focus (customizable via `:bind enter_manage_mode ...`) |
| `Alt+Z` | Global (`WRITE` / `MANAGE`) | Hide or show sidebar (`copy zoom`) |
| `Alt+1..9` | Global (`WRITE` / `MANAGE`) | Jump directly to root thread `1..9` (`AltGr`-safe) |
| `Shift+↑/↓/←/→` | `WRITE` (non-alt-screen) | Step prev/next sibling (`↑`/`↓`), ascend to parent (`←`), or descend to child (`→`) |
| `A` / `/agent` | `MANAGE` / `Composer` | Open `Select Agent` modal (`Enter` sibling, `s` switch, `d` default), or spawn directly via `/agent <provider>` / `:<provider>` |
| `?` | `MANAGE` (or `/?` in Composer) | Open Help modal with live `/` fuzzy filter |
| `:` | `MANAGE` | Open Sidebar Command Palette (`:agent`, `:<provider>`, `:theme`, `:sidebar`, `:bind`, …) |
| `Tab` | `MANAGE` / `WRITE · AGENT` | Switch `ᛇ Grein` ↔ `⎇ Worktrees` (`MANAGE`), or accept ghost text (`Composer`) |
| Mouse drag / 2× / 3× | Right pane | Pane-scoped select & auto-copy (`Alt+drag` overrides PTY mouse; middle-click pastes) |

> **macOS Terminal Tip:** Enable *Use Option as Meta key* in Terminal.app/iTerm2 for native `Alt+` shortcuts, or press `Ctrl+G` to enter `MANAGE` mode.

---

## CLI & Headless Commands

```bash
# Interactive modes
grein [--agent]                          # Terminal-first ($SHELL) or ACP agent root
grein standalone                         # Force in-process engine
grein daemon [--socket <SOCKET>]         # Run background daemon
grein attach [--socket <SOCKET>]         # Attach TUI client to running daemon

# Headless inspection & diagnostics
grein doctor [--json]                    # Check git, $SHELL, ACP agents, terminal env, DBs
grein status [--json] [--socket <SOCKET>] [--db <DB>]
grein threads [--json] [--db <DB>]
grein agents [--json] [--config <CONFIG>]

# Portable global preferences (~/.config/grein/grein.db or $GREIN_CONFIG_DB)
grein settings export [FILE] [--format <toml|json>] [--only <SECTIONS>]
grein settings import <FILE> [--format <auto|toml|json>] [--only <SECTIONS>] [--replace]
grein theme export [FILE] [--format <toml|json>]
grein theme import <FILE> [--format <auto|toml|json>] [--replace]
```

* Omit `[FILE]` or pass `-` to stream exports to stdout; pass `-` as `<FILE>` to read imports from stdin.
* `--only` accepts comma-separated sections (`theme,keybinding,ui`); imports apply merge-patch updates unless `--replace` is set.

---

## Documentation

- [`docs/FEATURES.md`](./docs/FEATURES.md) — Deep dive into Grein sessions, worktrees, embedded PTY, autocomplete, themes, and copy/paste
- [`docs/KEYBINDINGS.md`](./docs/KEYBINDINGS.md) — Complete mode-by-mode keybinding & customization reference
- [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) — Hexagonal architecture, in/out ports, driver/plugin registry, and persistence
- [`docs/demo.tape`](./docs/demo.tape) — Reproducible VHS demo recording script

---

## Building & Verification

```bash
cargo check             # Type-check all workspace crates
cargo test              # Run unit & integration test suites
cargo build --release   # Build release binary at target/release/grein
```

Workspace crates: `grein-core` (DAG & prefs), `grein-acp` (ACP stdio driver), `grein-workspace` (worktrees, shadows & PTY), `grein-tui` (Ratatui UI), and `grein-cli` (`grein` binary).
