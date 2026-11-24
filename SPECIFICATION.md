# grein — Documentation & Specification Index

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

The specification for **grein** has been split from a single monolithic file into focused living documents maintained alongside the workspace crates:

- **[`README.md`](./README.md)** — Quickstart, installation (Homebrew, Nix Flake, Cargo), launch modes, essential shortcuts, and CLI flags.
- **[`docs/FEATURES.md`](./docs/FEATURES.md)** — Complete feature guide covering `ᛇ Grein` DAG sessions, branch-named `⎇ Worktrees` & non-Git shadow snapshots, embedded `>_` PTY terminals, Composer 3-tier autocomplete, 16 live-preview themes, fuzzy help, pane-scoped copy/paste, and global preferences.
- **[`docs/KEYBINDINGS.md`](./docs/KEYBINDINGS.md)** — Full mode-by-mode keymap (`WRITE` vs. `MANAGE`), slash/palette commands, `:bind`/`:unbind` customization, and terminal compatibility rules.
- **[`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md)** — Hexagonal architecture across `grein-core`, `grein-acp`, `grein-workspace`, `grein-tui`, and `grein-cli`, including inbound/outbound ports, client-daemon JSON-RPC, and SQLite DAG/preference schemas.

---

## Workspace Crates & State

- [`crates/grein-core`](./crates/grein-core) — Pure-tree DAG model, `.grein/session.db` SQLite persistence, anchor parser, and global `UserPrefsStore` (`~/.config/grein/grein.db` or `$GREIN_CONFIG_DB`).
- [`crates/grein-acp`](./crates/grein-acp) — Agent Client Protocol (ACP) JSON-RPC stdio driver and permission policy gate.
- [`crates/grein-workspace`](./crates/grein-workspace) — Branch-named Git worktree allocator/diff engine, non-Git `.grein/shadow/` snapshot fallback, and VT100 PTY runner (`GREIN=1`).
- [`crates/grein-tui`](./crates/grein-tui) — Ratatui 2-mode TUI, `ᛇ Grein` & `⎇ Worktrees` views, customizable keymap, themes, and selection/clipboard engine.
- [`crates/grein-cli`](./crates/grein-cli) — Unified `grein` binary (standalone, daemon/attach, headless inspection, `doctor`, and `settings`/`theme` CLI).

---

## Demo & Verification

- Reproducible terminal walkthrough: [`docs/demo.tape`](./docs/demo.tape) (`vhs docs/demo.tape`).
- Workspace check & tests: `cargo test` (`flake_test` validates packaging and specification naming invariants).
