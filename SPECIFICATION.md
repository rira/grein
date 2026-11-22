# grein — Agentic TUI Manager Specification

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

## 1. Executive Summary & Problem Definition

### 1.1 The Problem
Modern agentic coding workflows present a severe friction point:
* **Linear Context Entanglement:** Agent interactions default to a single chronological list of turns. When an agent produces a multi-part answer (e.g., headings `#### 1.` and `#### 2.`), pursuing topic 1 while topic 2 is pending results in context contamination, token waste, and incoherent agent instruction following.
* **Terminal Isolation Drawback:** Opening independent terminal windows isolates the agent context completely. The secondary session lacks awareness of codebase analysis, decisions, and system prompts established in the primary session.
* **Synchronous Human Blocking:** Developers are blocked from drafting thoughts or exploring alternate sub-tasks while an agent executes long-running tools or streams responses.

### 1.2 The Solution
A dedicated **Agentic TUI Manager** built around a **Pure-Tree Directed Acyclic Graph (DAG) Context Model** paired with a **Two-Tier Client-Daemon RPC Architecture**.
* Conversations fork cleanly off structural anchors or active turns (`parent_thread_id` lineage), or start as blank independent roots (`/root`, `/new`).
* Child branches inherit their ancestor trunk up to the fork point without interleaving sibling turns, while separate roots remain completely isolated until manually joined onto a parent via `/join` / `/merge`.
* Non-blocking 2-column TUI allows browsing branches with live transcript previews, scrolling globally, drafting per-context prompts, and reviewing background tool/subagent executions simultaneously.

---

## 2. Core Architectural Pillars

### 2.1 Pillar I: The Conversation DAG Engine
Conversations are not flat lists or globally shared scratchpads; they form a Git-like forest of trees of immutable turn nodes.

```
   [Root 1 (`ᛉ`): Project Architecture]            [Root 2 (`ᛉ`): Blank Independent Root]
                    │                                                 │
        [Turn 1..N: Baseline Analysis]                       [Isolated Turns...]
          ├── Anchor [1]: Database Schema
          └── Anchor [2]: Ingress Routing
                    │
        ┌───────────┴───────────┐
        ▼                       ▼
  [Branch A]              [Branch B]         (explicit `parent_thread_id` = Root 1)
  (DB Schema)             (Ingress Routing)
        │                       │
        │ /join (`m`)           │ (sibling turns remain strictly isolated)
        ▼
  [DecisionLandmark] committed directly onto Root 1's active leaf
  (delivered append-only as `[Joined Context]` if Root 1 has a live ACP session)
```

#### Node & Lineage Structure
Each message node represents an immutable conversational event (`ConversationNode`):
* `id` (`NodeId`): UUIDv7 (time-sortable).
* `parent_id`: `Option<NodeId>` pointing to the preceding turn or fork point.
* `role` (`AuthorRole`): `User`, `Assistant`, `System`, or `ToolResult`.
* `content`: Full textual content (including `DecisionLandmark` commits).

Each branch head (`Thread`) tracks explicit tree lineage and lifecycle state:
* `id` (`ThreadId`), `parent_node_id` (`NodeId`), `active_leaf_id` (`NodeId`), `anchor_id` (`AnchorId`).
* `parent_thread_id`: `Option<ThreadId>` (`None` for top-level roots; explicit parent thread ID for forked child branches).
* `kind` (`ThreadKind`: `Agent` or `Terminal`), `cwd` (`Option<String>`, inherited from parent or assigned via opt-in `/worktree` or `/term [path]`).
* `title`, `status` (`ThreadStatus`), `acp_session_id` (`Option<String>`), `merged` (`bool`, rendered as `[joined]` when committed back to parent).

#### Anchor Extraction & Filtering Algorithm
To provide rapid branching without cluttering the interface with trivial step-by-step checklists, the parser applies structural heuristics:
1. **Markdown Headings:** All `#`, `##`, `###`, `####` elements are automatically registered as Primary Anchors.
2. **Titled Numbered Lists:** Numbered items matching `^([0-9]+)\.\s+(\*\*[^*]+\*\*|[^:\n]+:|^[\n]{20,})` are registered as Secondary Anchors (e.g. `1. **Database Schema**:` or `1. API Gateway Routing:`). Trivial short items (e.g. `1. Run tests`, `2. Commit code`) are filtered out to prevent anchor spam.
3. **Manual Branch Forking & Blank Roots:** Users can fork a child branch from the active context via `Ctrl+N`, `/fork`, `/branch`, or `/grein`, or create a blank independent root context (`parent_thread_id = None`) via `Alt+N`, `/root`, `/new`, or `n` in `ᛇ Grein`.

#### Branch Context Resolution
When a prompt is dispatched for Thread $K$ branching from Anchor $A$ at Turn $N$:
$$\text{Context Payload} = \text{RootSystemPrompt} + \text{TrunkPath}(Root \to N) + \text{AnchorHeader}(A) + \text{BranchTurns}(N \to K)$$
Turns present on sibling branches or separate top-level roots are strictly excluded from Thread $K$'s prompt, and no global Blackboard scratchpad is injected. Context remains focused, bounded, and KV-cache-friendly.

---

### 2.2 Pillar II: Client-Daemon RPC Architecture (Approach C)
To ensure long-term stability and clean separation of concerns, the system splits into two independent tiers:

```
┌─────────────────────────────────────────────────────────────┐
│                 Tier 1: TUI Client (Ratatui)                │
│  - Non-blocking input loop & event multiplexer              │
│  - `ᛇ Grein` context tree sidebar & live Transcript preview │
│  - Full-width Markdown, GFM table & ANSI PTY log renderer   │
│  - Per-context draft persistence & global scroll viewports  │
└──────────────────────────────▲──────────────────────────────┘
                               │
               IPC over Local Unix Domain Socket
               (Newline-delimited JSON-RPC 2.0 + event stream)
                               │
┌──────────────────────────────▼──────────────────────────────┐
│                  Tier 2: Agent Engine Daemon                │
│  - Context DAG persistence (SQLite via rusqlite)            │
│  - ACP stdio / LLM gateway & prompt context compiler        │
│  - PTY command execution & subagent watcher                 │
│  - Workspace isolation & worktree lease manager             │
└─────────────────────────────────────────────────────────────┘
```

#### Protocol Invariants
* **Non-Blocking UI:** The TUI client never calls LLMs, executes bash processes, or writes directly to disk. It is a pure terminal presentation and input client.
* **Resilience:** If the TUI process closes, crashes, or is detached, the daemon continues running agent streams, tool calls, and background jobs. Re-opening the TUI reconnects and re-hydrates state instantly.
* **Standardized RPC Methods:**
  * `session.create_root_thread(title?)` $\to$ `ThreadSummary`
  * `session.fork_thread(source_thread_id | parent_node_id, title?)` $\to$ `ThreadSummary`
  * `session.rename_thread(thread_id, title)` $\to$ `ThreadSummary`
  * `session.send_turn` / `session.send_thread_turn(thread_id, prompt)` $\to$ `TurnDispatchResult`
  * `session.merge_thread(thread_id, target_thread_id?, summary?)` $\to$ `ThreadSummary`
  * `session.delete_thread(thread_id)` $\to$ `next_active_thread_id`
  * `session.create_terminal(parent_thread_id?, title?, cwd?, rows, cols)` $\to$ `ThreadSummary`
  * `pty.write(thread_id, bytes)`, `pty.resize(thread_id, rows, cols)`, `pty.snapshot(thread_id, rows, cols)` $\to$ `PtyScreenSnapshot`
  * `workspace.ensure_thread_worktree(thread_id)` $\to$ `cwd` (opt-in isolated worktree assignment)
  * `session.cancel(thread_id)`, `session.shell(thread_id, command)`, `permissions.resolve`
*   *`events`: `StreamChunk`, `ThoughtChunk`, `ToolInvoked`, `ToolResult`, `ToolActivityUpdated`, `ShellOutput`, `PtyScreenUpdated`, `PtyExited`, `PermissionRequested`, `ThreadCreated`, `ThreadTitleUpdated`, `ThreadMerged`, `ThreadMetadataUpdated`, `ThreadDeleted`, `ThreadStatusChanged`, `AgentMetaUpdated`, `AgentConfigUpdated`, `AgentCommandsUpdated`, `SubagentUpdated`, `SubagentRemoved`, `Error`.

---

### 2.3 Pillar III: Workspace Isolation & Git-Aware Graceful Degradation

The tool has **zero hard Git requirement**. It functions in any directory, whether inside a Git repository or in an arbitrary scratch folder. The Core Conversation DAG, ACP agent communication, non-blocking Ratatui interface, and multi-thread switching are 100% decoupled from Git.

When file mutations occur (`write_file`, `edit_file`, build commands), the daemon dynamically selects an isolation strategy based on environment inspection:

| Workspace Mode | Detection Condition | Read Operations | Mutating Operations | Teardown / Merge Mechanism |
| :--- | :--- | :--- | :--- | :--- |
| **Mode 1: Git-Backed Worktree** | `.git` root detected in current or parent directory | Primary repository checkout | Dynamic `git worktree` (`.grein/worktrees/thread-<id>`) | Clean: auto-prune. Dirty: prompt `[Keep/Discard/Merge]`. Merges via Git cherry-pick. |
| **Mode 2: Standalone / Non-Git** | No `.git` found, or `git` binary absent from `$PATH` | Primary working directory | **Snapshot Shadowing:** Hardlink/reflink copy into `.grein/shadow/thread-<id>/` | Interactive diff review via embedded pure-Rust line diff engine; copy back to root on merge. |

#### Mode 1: Git Worktree Workflow (When in a Git Repo)
1. Thread starts in shared inspection mode against the primary checkout.
2. On first write tool call, daemon runs `git worktree add -b tui-thread-<id> .grein/worktrees/thread-<id> HEAD`.
3. Tool executes inside the isolated worktree directory.
4. On thread close:
   * **Clean Worktrees:** Pruned automatically (`git worktree remove`) with zero friction.
   * **Dirty Worktrees:** Prompt modal: `[k] Keep branch/worktree (archive) | [d] Discard & prune | [m] Merge changes into main workspace`.

#### Mode 2: Standalone Snapshot Workflow (When outside Git)
1. Thread reads live files in current directory.
2. On first write tool call, daemon clones modified target files or subtrees into an ephemeral shadow directory using filesystem hardlinks (`cp -al` on Linux / APFS clonefile on macOS) to avoid storage duplication.
3. Changes are compared against primary files using an embedded pure-Rust line diff engine.
4. Merging writes the approved shadow files back to the primary directory.

---

### 2.4 Pillar IV: Pure-Tree Lineage, Multi-Root Isolation & Manual Join
Rather than mutating a global shared Blackboard across unrelated tasks, **grein** enforces strict Git-like tree boundaries so prompt prefixes remain stable and free of cross-branch contamination:

1. **Explicit `parent_thread_id` Lineage & Sibling Isolation:**
   * Every forked branch records an explicit `parent_thread_id` alongside its `parent_node_id` fork anchor.
   * Branches inherit ancestor turns along `Root -> parent_node_id`, while sibling branches remain completely invisible to one another during context compilation.
   * Global Blackboard working memory is not injected into prompts, avoiding prompt prefix invalidation and cross-talk.
2. **Independent Blank Roots & In-Place Renaming:**
   * Create a blank top-level root (`parent_thread_id = None`) at any time via `/root [title]`, `/new [title]`, `Alt+N`, or `n` in `ᛇ Grein`. Each root starts a fresh DAG tree with zero shared context from other roots.
   * Rename the active or selected context via `/rename <title>`, `Alt+R`, `F2`, or `r` in `ᛇ Grein`.
3. **Manual Branch Join (`/join`, `/merge`, `m` in `ᛇ Grein`):**
   * When a branch finishes an investigation or sub-task, the user manually joins it into its parent branch (resolved via `parent_thread_id`, or an explicit target) using `/join [summary]`, `/merge [summary]`, or `m` on the selected branch in `ᛇ Grein`.
   * The engine commits a synthesized `DecisionLandmark` node (`AuthorRole::System`, `[Decision Landmark (Thread <id> - <topic>)]:\n<summary>`) directly onto the parent branch's `active_leaf_id` and marks the child `merged = true` (`[joined]` badge in `ᛇ Grein`).
   * **KV-Cache-Preserving Append-Only Delivery:** If the parent branch already holds a live ACP session, the joined summary is delivered on the parent's next turn as an append-only `[Joined Context]` note rather than rewriting the system prompt or restarting the session—preserving LLM KV caches while isolating sibling branches and independent roots.
4. **Branch Closing & Reparenting (`/close`, `/delete`, `d` / `x` in `ᛇ Grein`):**
   * Deleting a context (permitted whenever more than one context exists) removes it cleanly and reparents any nested children to the deleted branch's `parent_thread_id` (or promotes them to top-level roots if none).

---

### 2.5 Pillar V: Pluggable Agent Driver SPI & Agent-Agnostic Core

The system is strictly **agent-agnostic**: it has **zero default agent or vendor preference**.

A critical reality of modern developer tooling is that CLI agents (such as Google Antigravity `agy`, Anthropic `claude`, or OpenAI ACP agents) do not provide or require raw API keys. Instead, they authenticate via **enterprise SSO, Google Workspace login, or web OAuth**, storing credentials in local session caches (`~/.gemini/`, `~/.claude/`, system keychains).

To support any installed agent CLI while allowing direct API connections, the daemon defines a pluggable **`AgentDriver` SPI**:

```
┌─────────────────────────────────────────────────────────────┐
│                 Agent Engine Daemon (Tier 2)                │
└──────────────────────────────┬──────────────────────────────┘
                               │
                       trait AgentDriver
                               │
         ┌─────────────────────┴─────────────────────┐
         ▼                                           ▼
┌──────────────────────────────┐   ┌──────────────────────────────┐
│  Driver 1: ACP Stdio Driver  │   │  Driver 2: Direct API Driver │
│     (Any ACP-compliant CLI)  │   │  (Ollama, Gemini, Anthropic) │
├──────────────────────────────┤   ├──────────────────────────────┤
│ - Launches configured agent  │   │ - Direct HTTP/SSE streaming  │
│ - Reuses user SSO/login      │   │ - Uses env vars / API keys   │
│ - Zero API key setup         │   │ - Local offline inference    │
│ - JSON-RPC over stdin/stdout │   │   via Ollama / vLLM          │
└──────────────────────────────┘   └──────────────────────────────┘
```

#### Zero Default Agent & Configuration Discovery
* **Discovery on First Run:** If no configuration exists, the manager inspects `$PATH` for known ACP agents (e.g. `agy`, `claude`) and prompts the user to select their preferred agent command, or accepts a custom command.
* **Declarative Configuration (`config.toml`):**
  ```toml
  # ~/.config/grein/config.toml
  [agent]
  # Configured command to spawn the ACP stdio server
  command = ["agy", "acp", "--stdio"]
  protocol = "acp"

  # Or alternatively for other agents:
  # command = ["claude", "--acp"]
  # command = ["ollama", "run", "llama3"]
  ```

#### Driver 1: Agent Client Protocol (ACP) Subprocess
* **Zero API Key Requirement:** Directly spawns the configured agent CLI in ACP server mode.
* **Authentication Inherited:** Reuses the user's existing authenticated login state automatically. If unauthenticated, triggers the agent's interactive terminal auth flow.
* **Session Multiplexing:** Utilizes native session management or isolated worker subprocesses (`--process-per-session`) to bind parallel DAG branches to distinct execution contexts.
* **Structured Event Streaming:** Receives typed JSON-RPC streaming tokens, thoughts/reasoning deltas, and tool invocation requests over stdio without terminal scraping.

#### Driver 2: Direct API & Local LLM Driver
* Connects directly to cloud LLM endpoints via API keys (`GEMINI_API_KEY`, `ANTHROPIC_API_KEY`) or local inference engines (`http://localhost:11434/v1` for Ollama).
* Intended for air-gapped environments, local model experimentation, or custom enterprise proxies.

---

### 2.6 Pillar VI: Tool Permission Gate & Declarative Security Policy

To prevent rogue commands while eliminating prompt fatigue during multi-step tasks, the daemon integrates a two-stage security policy engine:

#### 1. Interactive TUI Permission Prompt
When an agent invokes a tool (e.g., executing a bash command or writing a file) that is not pre-authorized by policy, the TUI queues a centered permission modal while preserving background typing immunity:

```
┌────────────────────────────────────────────────────────────────────────┐
│ Permission Required                                                    │
│                                                                        │
│ Run cargo test --lib in .grein/worktrees/thread-8f2a/                  │
│                                                                        │
│ [y] Allow  [n] Deny                                                    │
└────────────────────────────────────────────────────────────────────────┘
```
(`y`, `Y`, or `Enter` approves; `n`, `N`, or `Esc` denies.)

#### 2. Declarative Allowlist Policy Files
The policy engine evaluates security rules hierarchically from two files:
1. **Global User Policy:** `~/.config/grein/policy.toml`
2. **Project-Local Policy:** `.grein/policy.toml` (committed in repo or git-ignored)

```toml
# Example .grein/policy.toml
[policy]
# Automatically allow read-only inspection tools without interactive prompts
auto_allow_read_tools = true  # read_file, grep, find, git status, git log

# Command prefix allowlist for bash/run_command
allow_commands = [
    "cargo check",
    "cargo test*",
    "git diff*",
    "npm test"
]

# Strict blocklist (always prompt or reject outright)
deny_commands = [
    "rm -rf *",
    "git push --force*",
    "git reset --hard*"
]

[worktree]
# Auto-approve file writes IF the thread is running inside an isolated worktree
auto_allow_writes_in_worktrees = true
```

---

### 2.7 Pillar VII: Concurrency, Rate Limiting & Focus Invariance

#### 1. Non-Blocking Concurrency Scheduler
When parallel threads are active (e.g., Thread 1 running an agent query while the user is typing in Thread 2):
* **Execution Queue & Limits:** The daemon enforces an adjustable concurrency limit (`max_concurrent_agents = 2` by default) to prevent rate-limit throttling from external LLMs or local GPU memory thrashing.
* **Focused Thread Priority:** The actively viewed thread always receives priority scheduling over background threads.
* **Independent Stream Channels:** Multiple threads can stream tokens simultaneously in the background without cross-contaminating UI state.

#### 2. Strict Focus Non-Stealing Invariant
A notorious flaw in terminal multiplexers and IDEs is popups stealing focus while the user is actively typing.
* **Typing Immunity:** When the user is composing text in `Composer`, incoming stream completions, finished tool runs, or background thread updates **never steal keyboard focus**.
* **Non-Intrusive Indicators:** Thread activity and permission alerts surface as status glyphs (`●`, `⚠`) in the `ᛇ Grein` sidebar and live indicators in the bottom status bar. Keyboard focus moves strictly on explicit user command (`Esc` or `Ctrl+G`).

---

### 2.8 Pillar VIII: Session Persistence & Multi-Run Resume

#### 1. Storage Location
All conversation DAG nodes, thread lineage metadata, and anchors are stored transactionally in a local embedded SQLite database (`rusqlite` in WAL mode):
* **Project-Local Mode (Default):** `.grein/session.db` in the repository root (added to `.gitignore`).
* **Global Mode (Fallback for non-git directories):** `~/.local/share/grein/projects/<repo-hash>/session.db`.

#### 2. SQLite Schema & Legacy Compatibility
* `nodes`: Immutable conversation turns (`id`, `parent_id`, `role`, `content`, `created_at`), including synthesized `DecisionLandmark` nodes committed on `/join`.
* `threads`: Branch heads and explicit DAG lineage (`id`, `parent_node_id`, `anchor_id`, `active_leaf_id`, `status`, `created_at`, `title`, `acp_session_id`, `merged`, `parent_thread_id`).
* `anchors` & `node_anchors`: Extracted structural anchors linked to turn nodes.
* `blackboard_entries`: **Legacy compatibility table** retained read-compatible in the schema for safe migration and opening of older `.grein/session.db` files; **unused by pure-tree context compilation**.

#### 3. State Hydration on Launch
* Launching `grein` in a repository immediately resumes the active session, restoring all roots, branches (`parent_thread_id` hierarchy), `[joined]` status, unmerged worktrees, and input drafts.
* If a previous run was abruptly killed (e.g. terminal crash), the SQLite WAL journal recovers state cleanly and cleans up orphaned PIDs.

---

## 3. Terminal User Interface (TUI) Specification

### 3.1 Screen Layout Breakdown
The interface uses a **pure 2-column responsive layout** powered by Ratatui—with **no Blackboard side pane or Blackboard focus**—paired with a bottom `Composer`, an optional auto-hiding `Subagents` strip, and a 1-line status bar:

```
┌───────────────────────────┬───────────────────────────────────────────────────┐
│ ᛇ Grein (34%)             │ Transcript (66% — or Transcript (Preview: <title>))│
│                           │                                                   │
│ ᛉ Main                    │ ❯ Design ingress DTOs and database schema         │
│ ├─ ● DB Schema            │                                                   │
│ │  └─ Auth Tokens         │ ## Agent (agy)                                    │
│ └─ Ingress API [joined]   │ > ↳ Joined from Ingress API: Use pure DTOs...     │
│ ᛉ Scratchpad              │                                                   │
│                           │ $ cargo check (exit 0)                            │
│                           │ > ✓ `read_file`: crates/grein-core/src/model.rs   │
├───────────────────────────┴───────────────────────────────────────────────────┤
│ Composer (auto-growing 3–10 lines, per-context preserved draft)               │
│ > Type message, /command, or !shell... (Esc/Ctrl+G to toggle ᛇ Grein focus)   │
├───────────────────────────────────────────────────────────────────────────────┤
│ Subagents (auto-hides when empty; up to 5 active rows + overflow)             │
├───────────────────────────────────────────────────────────────────────────────┤
│ Ctrl+G tree · Alt+T zsh · ? help           agent · default · ✓ idle · 0 subagents │
└───────────────────────────────────────────────────────────────────────────────┘
```

#### Left Column (`34%` width): `ᛇ Grein` Context Tree Sidebar
* Displays the full multi-root mixed Agent + Terminal forest: top-level Agent roots marked with `ᛉ`, Terminal nodes marked with `>_` (plus `[exit N]` / `[exited]` badge when terminated), and child branches nested depth-first via `parent_thread_id` with Unicode box-drawing connectors (`├─`, `└─`, `│`).
* Real-time status glyphs (`●` streaming/executing/thinking, `⚠` permission required or error) and `[joined]` suffix on merged branches.
* **Live Preview on Browse:** Moving the cursor (`↑`/`↓`, `j`/`k`) in `ᛇ Grein` immediately previews that context's transcript (`Transcript (Preview: <title>)`) or live VT100 terminal screen (`Terminal (Preview: <title>)`, fetching `pty_snapshot` if missing and hiding the Agent `Composer`) while keeping focus in `ᛇ Grein` until confirmed with `Enter`, `l`, `→` (`Right`), `Esc`, `Ctrl+G`, or `Tab` (on a terminal node).

#### Right Column (`66%` width, or `100%` Borderless Copy Zoom): Dual-Mode `Transcript` / Live `Terminal` Pane
* **Agent Mode (`ThreadKind::Agent`):** Full-width Markdown rendering including headings, inline formatting, syntax-highlighted code fences, GFM tables, horizontal rules, leaf-green `❯ ` user prompt prefix, lineage banners (`> ↳ Forked from <parent> (<N> ancestor turns in LLM context)`), join markers (`↳ Joined from <branch>`), collapsible tool/shell cards (`Ctrl+O` toggles all), and ANSI-colored PTY shell blocks (`$ cmd (exit N)`).
* **Terminal Mode (`ThreadKind::Terminal`):** Hides `Composer` completely and expands the right pane to full height above the status bar (or `100%` full-screen width when zoomed via `Ctrl+T`), rendering `PtyScreenSnapshot` cell-by-cell (`Default`/`Indexed`/`Rgb` colors, bold/italic/underline/inverse, wide UTF-8 cells skipping `is_wide_continuation`, and clamped cursor positioning when `FocusedPane::Terminal` is active). Cold reopens fetch `pty_snapshot` automatically, pane dimension changes emit `pty_resize`, and PTY keystrokes are serialized over a single FIFO writer channel. Once a PTY exits, `Esc` (as well as `Ctrl+G`) returns to `ᛇ Grein` without forwarding further keys to `pty.write`.
* **Borderless-Sides Copy Zoom (`Ctrl+T`, `z` in Tree, `/zoom`):** Collapses `ᛇ Grein` and expands the right pane to `100%` width with `Borders::TOP | Borders::BOTTOM` only (zero left/right `│` borders) for clean terminal mouse drag-select copying, complemented by `y` / `Alt+Y` / `/copy [code|all]` (copying `PtyScreenSnapshot::plain_text()` on terminal threads via native + OSC 52 clipboard). Entering `ᛇ Grein` (`Ctrl+G`, `Esc`) automatically unhides the sidebar.
* **Global Transcript Scrolling:** Independent scrollback buffer per context (`TranscriptViewport`) using exact pre-wrapped styled lines (zero bottom clipping), controllable globally from anywhere in the UI via `Alt+↑` / `Alt+↓`, `PgUp` / `PgDn`, and `Alt+Home` / `Alt+End`, or zero-modifier Tree keys (`Ctrl+U` / `Ctrl+D`, `g` / `G`)—including while live-previewing another branch in `ᛇ Grein`.
* **External `$PAGER` Diff Integration:** Pressing `o` when inspecting a diff card suspends Ratatui and opens the colorized diff in `$PAGER` (e.g. `delta`, `git diff`, `less`), cleanly restoring the TUI on exit.

#### Bottom Area: `Composer`, Optional `Subagents` Strip & Status Bar
* **`Composer`:** Soft-wraps single and multi-line drafts to inner cell width, auto-grows from 3 to 10 lines following the active cursor row, preserves an independent input draft per context, supports full Readline editing/kill-ring shortcuts (`Ctrl+A`/`E`, `Ctrl+F`/`B`, `Alt+B`/`F`), `!cmd` PTY shell execution, and slash commands including interactive `/model`, `/mode`, `/thinking` (`/effort`), `/commands`, and terminal aliases (`/term`, `/terminal`, `/zsh`, `/sh`, `/shell`, `/bash`).
* **`Subagents` Strip:** Auto-hiding panel below `Composer` (renders only when subagents are active on the focused context, up to 5 rows + `+N more` overflow) showing live subagent type, role, model, activity, and elapsed time.
* **1-Line Status Bar:** Left side displays `Ctrl+G tree · Alt+T zsh · ? help` (or `Esc cancel · Ctrl+G tree · Alt+T zsh` while streaming/executing, `Ctrl+G for tree` in live `Terminal`, and `exited · Esc/Ctrl+G tree` in an exited `Terminal`) plus status notices; right side displays `<mode> · <model> · <live_status> · <N subagents>`.

---

### 3.2 Compact Keyboard Navigation & Commands (`Option 1`)

#### Focus, Tree Navigation & Global Transcript Scrolling

| Keybinding | Scope | Action |
| :--- | :--- | :--- |
| `Esc` | Global (outside live `Terminal`) | Cancel active streaming/executing turn; otherwise toggle focus between `Composer`/`ᛇ Grein` (auto-unhides sidebar; in `ᛇ Grein`, activates the selected node into `Composer` or `Terminal`). Inside a live `Terminal`, `Esc` passes `\x1b` to PTY; on an **exited** `Terminal`, `Esc` returns to `ᛇ Grein` |
| `Ctrl+G` | Global (incl. `Terminal`) | Toggle focus between `Composer`/`Terminal` and `ᛇ Grein` sidebar (auto-unhiding sidebar when zoomed; activating the selected node when in `ᛇ Grein`) without cancelling turns or forwarding `\x07` to PTY |
| `Alt+T` | Global (outside `Terminal`) | Open or jump straight to the active/selected thread's `>_` terminal (switches to an existing child/selected Terminal or spawns a new one; passes through to PTY inside `FocusedPane::Terminal`) |
| `Tab` / `Shift+Tab` | Global (outside `Terminal`) | Cycle branches within the active root's subtree (`Tab` in `ᛇ Grein` on a terminal node enters `Terminal`; inside `Terminal`, `Tab` sends `\t` to PTY) |
| `Alt+Shift+↑` / `Alt+Shift+↓` | Global | Switch to previous / next top-level root |
| `Alt+J` / `Alt+K` | Global | Switch to next / previous top-level root |
| `Alt+↑` / `Alt+↓` | Global | Scroll `Transcript` up / down (including live previewed branches when browsing `ᛇ Grein`) |
| `PgUp` / `PgDn` | Global | Page `Transcript` up / down (including live previewed branches) |
| `Alt+Home` / `Alt+End` | Global | Jump to top / bottom of `Transcript` (re-enables auto-scroll at bottom) |
| `Ctrl+O` | Global | Toggle all collapsible tool/shell cards on active or previewed transcript |
| `Ctrl+T` | Global | Toggle borderless-sides Transcript copy zoom (hides/shows `ᛇ Grein`) |
| `Alt+Y` | Global | Copy last assistant reply of active or previewed transcript to clipboard |
| `Ctrl+N` | Global | Fork a new child branch from the active context |
| `Alt+N` | Global | Create a new blank independent root context |
| `Alt+R` / `F2` | Global | Prefill `/rename <title>` for active (or `ᛇ Grein`-selected) context and focus `Composer` |
| `?` | Global (when `Composer` empty or outside `Composer`) | Open scrollable Keybindings & Commands help modal (`j`/`k`/`↑`/`↓` scroll) |
| `Ctrl+C` / `Ctrl+Q` | Global | Quit `grein` |
| `o` | Diff card | Open selected diff in external `$PAGER` |

#### `ᛇ Grein` Sidebar Keys (When Focused, Zero-Modifier Safe)

| Keybinding | Action |
| :--- | :--- |
| `↑` / `↓` or `k` / `j` | Move selection through the tree and live-preview in `Transcript` / `Terminal` |
| `J` / `K` | Switch to next / previous top-level root |
| `Ctrl+U` / `Ctrl+D` | Scroll active/previewed `Transcript` half-page up / down |
| `g` / `G` | Jump active/previewed `Transcript` to top / bottom |
| `Enter` / `l` / `→` (`Right`) | Activate selected branch/root (or anchor) and focus `Composer` (or `Terminal` for `>_` nodes) |
| `t` / `T` | Open a child terminal under the selected context (`t`, inherits `cwd`) or a blank root terminal (`T`) |
| `f` | Fork a new child branch off the selected context |
| `n` | Create a new blank independent root context |
| `r` | Prefill `/rename <title>` for the selected context and focus `Composer` |
| `m` | Manually join (`/join`) the selected branch into its parent branch |
| `d` / `x` | Close/delete the selected context (when >1 context exists) |
| `z` | Toggle borderless-sides Transcript copy zoom |
| `y` | Copy last assistant reply of selected/previewed context to clipboard |
| `q` | Quit `grein` |

> **OS / Terminal Portability Tip:** On macOS Terminal.app or iTerm2, configure Option as Meta/Esc+ for `Alt+` shortcuts; inside `tmux`, `Ctrl+B` in `Composer` performs Readline `backward-char` cleanly and OSC 52 passthrough (`\x1bPtmux;...`) copies directly to the host clipboard. On all platforms (including Swedish/European layouts and Wayland), zero-modifier `ᛇ Grein` keys (`J`/`K`, `g`/`G`, `Ctrl+U`/`D`, `f`, `z`, `y`) cover root switching, scrolling, branching, zoom, and copying without `Alt`.

#### `Composer` Readline & Editing Keys

| Keybinding | Action |
| :--- | :--- |
| `Enter` | Dispatch prompt, slash command, or `!cmd` shell command |
| `Shift+Enter` / `Ctrl+Enter` / `Alt+Enter` / `Ctrl+J` | Insert newline in `Composer` (Kitty disambiguate-escape enabled best-effort) |
| `Ctrl+A` / `Home` | Move cursor to start of line |
| `Ctrl+E` / `End` | Move cursor to end of line |
| `Ctrl+F` / `→` / `Ctrl+B` / `←` | Move forward / backward by one character |
| `Alt+←` / `Alt+→`, `Ctrl+←` / `Ctrl+→`, `Alt+B` / `Alt+F` | Move backward / forward by word |
| `Ctrl+K` / `Ctrl+U` | Kill to end of line / kill to start of line |
| `Ctrl+W` / `Alt+Backspace` | Kill previous word |
| `Alt+D` / `Ctrl+Delete` / `Alt+Delete` | Kill next word |
| `Ctrl+Y` / `Ctrl+D` | Yank from kill ring / delete character under cursor |

#### Slash Commands, ACP Config Picker & Shell Escape

| Command | Action |
| :--- | :--- |
| `!cmd` | Execute shell command in a PTY under the active context (`$ cmd (exit N)` with ANSI rendering) |
| `/term [path]` (`/terminal`, `/zsh`, `/sh`, `/shell`, `/bash`) | Spawn an interactive child PTY terminal (inheriting active context's `cwd` or overriding with `[path]`, case-insensitive) and focus `Terminal` |
| `/worktree` | Opt-in allocate/attach an isolated git worktree `cwd` for the active thread (inherited by child terminals) |
| `/fork [title]`, `/branch [title]`, `/grein [title]` | Fork a new child branch off the active context (`parent_thread_id`) |
| `/root [title]`, `/new [title]` | Create a new blank independent root context |
| `/rename <name>` | Rename the active or selected context |
| `/join [summary]`, `/merge [summary]` | Manually join active branch into its parent branch, committing a `DecisionLandmark` |
| `/close [target]`, `/delete [target]` | Close/delete the active or named context (>1 context required) |
| `/model [arg]`, `/mode [arg]`, `/thinking [arg]` (`/effort`) | Open interactive ACP config picker modal (`Tab`/`d` toggles branch vs default) or fuzzy-match & apply argument (`--default` / `default ` prefix supported) |
| `/commands` | List agent-advertised ACP slash commands (`AgentSlashCommand`) |
| `/copy [code\|all]` | Copy last assistant reply, latest fenced code block, or full plain transcript to clipboard |
| `/zoom` | Toggle borderless-sides Transcript copy zoom |
| `/?`, `/help` | Open the scrollable Keybindings & Commands help modal |
| `/quit` | Quit `grein` |

---

### 3.3 Thread Status Lifecycle & Symbolic Indicators

To give instant situational awareness across all active roots and branches without losing context:

| Indicator | Location | Color | Semantic Meaning |
| :--- | :--- | :--- | :--- |
| `ᛉ` | `ᛇ Grein` root row (when idle) | Green | Top-level independent root Agent context |
| `>_` | `ᛇ Grein` terminal row | Cyan / Dark Gray / Red | Interactive PTY terminal context (running / exited 0 / exited non-zero) |
| `[exit N]` | `ᛇ Grein` terminal suffix | Dark Gray / Red | Exit status badge rendered once a PTY process exits |
| `├─` / `└─` / `│` | `ᛇ Grein` child rows | Dark Gray | Depth-first tree connectors showing `parent_thread_id` hierarchy |
| `●` | `ᛇ Grein` row | Yellow | Context is actively `streaming`, `executing`, or `thinking` |
| `⚠` | `ᛇ Grein` row & Status Bar | Red | Context requires tool permission or hit an error |
| `[joined]` | `ᛇ Grein` row suffix | Dark Gray | Branch was manually joined into its parent via `/join` / `/merge` |
| `⠋` .. `⠏` | Status Bar | Yellow | Live animated spinner with active tool name or reasoning preview |
| `✓ idle` | Status Bar | Green | Active context is idle and ready for input |
| `●` / `✓` / `✖` | `Subagents` strip | Yellow / Green / Red | Live subagent status (`running` / `completed` / `failed`) |

#### Visual Rendering Example (`ᛇ Grein`):
```text
ᛉ Main
├─ ● DB Schema
│  └─ Auth Tokens
└─ Ingress API [joined]
ᛉ Investigation
└─ ⚠ Migration Check
```

---

## 4. Technical Stack & Implementation Boundary

* **Language:** Rust (2021 edition)
* **TUI Layer:** `ratatui` + `crossterm` + pure-Rust Markdown/GFM/ANSI renderer (`grein-tui`)
* **Concurrency Runtime:** `tokio` (multi-threaded async runtime)
* **Local Persistence:** `rusqlite` (embedded SQLite in WAL mode, zero external DB server)
* **Serialization & RPC:** `serde`, `serde_json`, Unix Domain Sockets (`tokio::net::UnixListener`)
* **Process Execution Model:** Native OS subprocess runner (`tokio::process::Command`) and PTY shell runner with `cwd` scoped to the dedicated worktree directory. Environment sanitization with explicit `PATH` pass-through. Zero container requirement (no Docker, no Podman).
* **Distribution Target:** Single static or self-contained binary packaged for Homebrew (`brew install`), Nix (`nix profile install` / Flake), and Cargo (`cargo install`).
* **Deployment Topology (Single Binary, Dual Mode):**
  * **Default (Standalone):** `grein` (or `grein standalone`) launches TUI and embeds the daemon engine internally within Tokio tasks. Zero socket setup required.
  * **Headless / Multi-Client:** `grein daemon` runs headless in the background; `grein attach` connects over a local Unix domain socket (`.grein/daemon.sock`).
  * **Headless Inspection Subcommands:** `grein threads`, `grein status`, and `grein agents --json` for scriptable and CLI introspection.

---

## 5. Verification & Specification Milestones

1. **Milestone 1:** Headless DAG Engine & Persistence (SQLite message DAG, explicit `parent_thread_id` lineage, anchor parser).
2. **Milestone 2:** Daemon RPC Protocol (Unix socket server, streaming event notifications, ACP & PTY execution loops).
3. **Milestone 3:** Pure 2-Column Ratatui Shell & Non-Blocking Multiplexer (`ᛇ Grein` tree sidebar, full-width `Transcript` with live preview & global scrolling, `Composer`, `Subagents` strip, compact Option 1 keymap).
4. **Milestone 4:** Lazy Worktree Allocator & Manual Join Engine (CoW worktrees, diff viewer, `DecisionLandmark` commit & `[Joined Context]` delivery).
