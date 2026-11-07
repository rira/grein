# grein

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

**grein** is a high-performance, agent-agnostic Terminal User Interface (TUI) manager built around **Directed Acyclic Graph (DAG) Context Trees**. It prevents context pollution when exploring parallel sub-topics with AI coding agents while sharing foundational project context.

---

## Architectural Pillars

1. **DAG Context Tree:** Messages are immutable graph nodes; threads branch off structural anchors (such as `#### 1. Database Schema` or `1. **Ingress API**`).
2. **Agent-Agnostic with ACP Protocol:** Interfaces directly with any installed CLI agent (e.g. `agy acp --stdio`, `claude --acp`) over standard JSON-RPC. Reuses existing SSO and local credentials with **zero API key requirement**.
3. **Workspace Isolation:** Dynamically uses isolated `git worktree`s for code-modifying threads while reads operate directly against the shared working directory. Gracefully falls back to directory shadow snapshots outside Git repositories.
4. **4-Pane Responsive TUI (Ratatui):** Non-blocking input composer, real-time thread status indicators (`⠋`, `⚙`, `⚠️`, `💬`, `✓`), external pager diff hook (`o`), and shared Blackboard memory.

---

## Workspace Structure

* `crates/grein-core`: Message DAG graph model, SQLite persistence, and heuristic anchor parser.
* `crates/grein-acp`: Agent Client Protocol (ACP) JSON-RPC stdio subprocess driver.
* `crates/grein-workspace`: Git worktree allocator and shadow snapshot fallback engine.
* `crates/grein-tui`: Ratatui terminal interface and non-blocking event multiplexer.
* `crates/grein-cli`: The unified executable (`grein`) supporting standalone and headless daemon modes.

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
