# grein — Agentic TUI Manager Specification

> **grein** *(Old Norse)*: Branch, bough, division, heading, or distinct line of thought.

## 1. Executive Summary & Problem Definition

### 1.1 The Problem
Modern agentic coding workflows present a severe friction point:
* **Linear Context Entanglement:** Agent interactions default to a single chronological list of turns. When an agent produces a multi-part answer (e.g., headings `#### 1.` and `#### 2.`), pursuing topic 1 while topic 2 is pending results in context contamination, token waste, and incoherent agent instruction following.
* **Terminal Isolation Drawback:** Opening independent terminal windows isolates the agent context completely. The secondary session lacks awareness of codebase analysis, decisions, and system prompts established in the primary session.
* **Synchronous Human Blocking:** Developers are blocked from drafting thoughts or exploring alternate sub-tasks while an agent executes long-running tools or streams responses.

### 1.2 The Solution
A dedicated **Agentic TUI Manager** built around a **Directed Acyclic Graph (DAG) Context Tree** paired with a **Two-Tier Client-Daemon RPC Architecture**.
* Conversations fork cleanly off structural anchors (such as markdown headings `#### 1. ...`).
* Threads share root context and global working memory without interleaving orthogonal discussion turns.
* Non-blocking TUI interface allows switching between threads, typing drafts, and reviewing background tool executions simultaneously.

---

## 2. Core Architectural Pillars

### 2.1 Pillar I: The Conversation DAG Engine
Conversations are not lists; they are trees of immutable turn nodes.

```
                  [Root Node: Project System Prompt]
                                  │
                  [Turn 1: Baseline Architecture]
                                  │
          [Turn 2 (Assistant): 3-Part Architecture Proposal]
             ├── Anchor [1]: Database Schema & Storage
             ├── Anchor [2]: Ingress Routing & DTOs
             └── Anchor [3]: Frontend Rune Binding
                                  │
            ┌─────────────────────┼─────────────────────┐
            ▼                     ▼                     ▼
     [Thread Branch 1]     [Thread Branch 2]     [Thread Branch 3]
     Target: Anchor [1]    Target: Anchor [2]    Target: Anchor [3]
     Turns: 1A, 1B, 1C     Turns: 2A, 2B         Turns: 3A
            │                     │
            └──────────┬──────────┘
                       ▼
            [Thread Convergence / Merge]
   (Concise synthesized decision committed back to Trunk)
```

#### Node Structure
Each message node represents an immutable conversational event:
* `node_id`: UUIDv7 (time-sortable).
* `parent_id`: `Option<UUID>` pointing to the fork or preceding turn.
* `role`: `User`, `Assistant`, `System`, or `ToolResult`.
* `content`: Full textual content.
* `anchors`: Extracted semantic anchors (heading level, title, slug, range).
* `metadata`: Token metrics, generation latency, tool invocation IDs.

#### Anchor Extraction & Filtering Algorithm
To provide rapid branching without cluttering the interface with trivial step-by-step checklists, the parser applies structural heuristics:
1. **Markdown Headings:** All `#`, `##`, `###`, `####` elements are automatically registered as Primary Anchors.
2. **Titled Numbered Lists:** Numbered items matching `^([0-9]+)\.\s+(\*\*[^*]+\*\*|[^:\n]+:|^[\n]{20,})` are registered as Secondary Anchors (e.g. `1. **Database Schema**:` or `1. API Gateway Routing:`). Trivial short items (e.g. `1. Run tests`, `2. Commit code`) are filtered out to prevent anchor spam.
3. **Manual Selection Forking (`Ctrl+N`):** Users can highlight any block of text or select any turn in the transcript and press `Ctrl+N` to manually fork a thread from that exact anchor.

#### Branch Context Resolution
When a prompt is dispatched for Thread $K$ branching from Anchor $A$ at Turn $N$:
$$\text{Context Payload} = \text{RootSystemPrompt} + \text{TrunkPath}(Root \to N) + \text{AnchorHeader}(A) + \text{BranchTurns}(N \to K)$$
Turns present in Thread Branch 2 or 3 are excluded from Thread Branch 1's prompt. Context remains focused, bounded, and free of cross-talk.

---

### 2.2 Pillar II: Client-Daemon RPC Architecture (Approach C)
To ensure long-term stability and clean separation of concerns, the system splits into two independent tiers:

```
┌─────────────────────────────────────────────────────────────┐
│                 Tier 1: TUI Client (Ratatui)                │
│  - Non-blocking input loop & event multiplexer              │
│  - Thread tree & anchor navigation pane                     │
│  - Streaming markdown & tool log renderer                   │
│  - Draft persistence per thread                             │
└──────────────────────────────▲──────────────────────────────┘
                               │
               IPC over Local Unix Domain Socket
               (Bidirectional JSON-RPC 2.0 / SSE)
                               │
┌──────────────────────────────▼──────────────────────────────┐
│                  Tier 2: Agent Engine Daemon                │
│  - Context DAG persistence (SQLite/Redb)                    │
│  - LLM API gateway & token budget tracker                   │
│  - Model Context Protocol (MCP) tool host                   │
│  - Workspace isolation & worktree lease manager             │
└─────────────────────────────────────────────────────────────┘
```

#### Protocol Invariants
* **Non-Blocking UI:** The TUI client never calls LLMs, executes bash processes, or writes directly to disk. It is a pure terminal presentation and input client.
* **Resilience:** If the TUI process closes, crashes, or is detached, the daemon continues running agent streams, tool calls, and background jobs. Re-opening the TUI reconnects and re-hydrates state instantly.
* **Standardized RPC Methods:**
  * `session.fork_thread(parent_node_id, anchor_id)` $\to$ `thread_id`
  * `session.send_turn(thread_id, content)` $\to$ `turn_id`
  * `session.merge_thread(thread_id, target_node_id, strategy)` $\to$ `merged_node_id`
  * `events`: `StreamChunk`, `ToolInvoked`, `ToolResult`, `AnchorDiscovered`, `WorktreePromoted`.

---

### 2.3 Pillar III: Workspace Isolation & Git-Aware Graceful Degradation

The tool has **zero hard Git requirement**. It functions in any directory, whether inside a Git repository or in an arbitrary scratch folder. The Core Conversation DAG, ACP agent communication, non-blocking Ratatui interface, and multi-thread switching are 100% decoupled from Git.

When file mutations occur (`write_file`, `edit_file`, build commands), the daemon dynamically selects an isolation strategy based on environment inspection:

| Workspace Mode | Detection Condition | Read Operations | Mutating Operations | Teardown / Merge Mechanism |
| :--- | :--- | :--- | :--- | :--- |
| **Mode 1: Git-Backed Worktree** | `.git` root detected in current or parent directory | Primary repository checkout | Dynamic `git worktree` (`.grein/worktrees/thread-<id>`) | Clean: auto-prune. Dirty: prompt `[Keep/Discard/Merge]`. Merges via Git cherry-pick. |
| **Mode 2: Standalone / Non-Git** | No `.git` found, or `git` binary absent from `$PATH` | Primary working directory | **Snapshot Shadowing:** Hardlink/reflink copy into `.grein/shadow/thread-<id>/` | Interactive diff review via pure-Rust `similar` crate; copy back to root on merge. |

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
3. Changes are compared against primary files using an embedded pure-Rust diff engine (`similar` crate).
4. Merging writes the approved shadow files back to the primary directory.

---

### 2.4 Pillar IV: Shared Context & Memory (The Blackboard)
While threads diverge in conversational turns, they often need to share critical facts (e.g., agreed port numbers, database schema decisions, architectural constraints).

1. **Shared Trunk Baseline:** All threads automatically inherit all turns up to the branch point.
2. **Global Working Memory (The Blackboard):**
   * A bounded markdown scratchpad managed by the daemon.
   * Automatically injected into the system prompt across all active threads.
   * Contains:
     * Active architectural decisions.
     * Established domain models and invariants.
     * Modified file registry across threads.
3. **On-Demand Thread Knowledge Import (`Ctrl+R` / `/import <thread>`):**
   * When working in Thread 2, the user can import conclusions from Thread 1 by typing `/import 1` or pressing `Ctrl+R`.
   * The daemon prompts the agent to synthesize an atomic decision turn from Thread 1:
     `[Context Import from Thread 1 (DB Schema): Finalized user table with columns id, email, created_at]`.
   * This turn is inserted into Thread 2's transcript, immediately informing Thread 2 without merging the entire conversation.
4. **Thread Merge & Synthesis:**
   * When a sub-topic concludes (e.g. `#### 1. Database Schema` is resolved), the user triggers `Merge to Trunk`.
   * The model synthesizes an atomic 2–3 line summary of the conclusion.
   * This summary is appended to the Trunk as a permanent landmark visible to all subsequent branches.

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
When an agent invokes a tool (e.g., executing a bash command or writing a file) that is not pre-authorized by policy, the active thread transcript halts tool execution and displays an inline permission card:

```
┌────────────────────────────────────────────────────────────────────────┐
│ ⚠️  Tool Execution Request: run_command                                │
│ Command: cargo test --lib                                              │
│ Working Directory: .grein/worktrees/thread-8f2a/ (Isolated)            │
├────────────────────────────────────────────────────────────────────────┤
│ [a] Allow Once   [d] Deny   [t] Always for Thread   [s] Always Session │
└────────────────────────────────────────────────────────────────────────┘
```

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
* **Typing Immunity:** When the user is composing text in the input box, incoming stream completions, finished tool runs, or permission requests **never steal keyboard focus**.
* **Non-Intrusive Indicators:** Alerts and permission requests display as badge counters or unread indicators on the thread tab. They only become interactive when the user presses `Esc` or cycles focus to the transcript.

---

### 2.8 Pillar VIII: Session Persistence & Multi-Run Resume

#### 1. Storage Location
All conversation DAG trees, branch points, blackboard entries, and worktree leases are stored transactionally in a local embedded SQLite database:
* **Project-Local Mode (Default):** `.grein/session.db` in the repository root (added to `.gitignore`).
* **Global Mode (Fallback for non-git directories):** `~/.local/share/grein/projects/<repo-hash>/session.db`.

#### 2. State Hydration on Launch
* Launching `grein` in a repository immediately resumes the active session, restoring all branches, unmerged worktrees, and input drafts.
* If a previous run was abruptly killed (e.g. terminal crash), the database journal recovers state cleanly and cleans up orphaned PIDs.

---

## 3. Terminal User Interface (TUI) Specification

### 3.1 Screen Layout Breakdown
The interface uses a 4-pane responsive layout powered by Ratatui:

```
┌─────────────────┬──────────────────────────────────────┬────────────────┐
│ [1] THREADS     │ [2] ACTIVE THREAD TRANSCRIPT         │ [3] BLACKBOARD │
│                 │                                      │                │
│ ▼ Trunk         │ # Ingress Routing & DTOs             │ ## Decisions   │
│   ├── [1] DB *  │                                      │ - Port: 8080   │
│   └── [2] API   │ [Agent 10:52]                        │ - DB: Postgres │
│       ├── (You) │ We should use pure Java records      │                │
│       └── (Ag.) │ for all ingress DTOs.                │ ## Worktrees   │
│                 │                                      │ - th-1: clean  │
│ [STREAMING th-1]│ > ran cargo check (exit 0)           │ - th-2: dirty  │
│ [IDLE      th-2]│                                      │                │
├─────────────────┴──────────────────────────────────────┴────────────────┤
│ [4] INPUT COMPOSER (Replying to: #### 2. Ingress Routing) [Alt+1 to sw] │
│ > Type message here... (Esc to normal mode, Ctrl+Enter to dispatch)     │
└─────────────────────────────────────────────────────────────────────────┘
```

#### Pane 1: Thread & Anchor Navigator (Left, ~22% width)
* Tree visualization of conversation trunks, active branches, and nested replies.
* Anchor badges extracted from headings and titled numbered items (`[1] DB Schema`, `[2] API Routes`).
* Real-time status glyphs (e.g. `⠋` streaming, `⚙` executing tool, `⚠️` permission required, `💬` awaiting input).
* Workspace dirty marker (`*`) indicating uncommitted code modifications in that thread.
* Unread message dot (`●`) for background turns completed while viewing another thread.

#### Pane 2: Active Thread Transcript (Center, ~56% width)
* Markdown syntax rendering with code block highlighting.
* Automatic breadcrumb trail: `Root > Milestone 1 > Anchor #### 2. API Routes`.
* Collapsible tool execution cards (bash runs, file edits, git status).
* **Diff Preview & External Pager Integration:**
  * When a thread modifies files, an inline diff card shows modified files and `+`/`-` line statistics.
  * Pressing `o` (or `Enter` when focusing the diff card) suspends Ratatui and opens the full colorized diff in `$PAGER` (e.g. `delta`, `git diff`, `less`), cleanly restoring the TUI upon exit.
* Independent scrollback buffer per thread (scrolling thread 2 does not reset thread 1).

#### Pane 3: Blackboard & Global Inspector (Right, ~22% width, Collapsible)
* Pinned architectural invariants and cross-thread decisions.
* Worktree registry: shows active isolated worktrees and modified file counts.
* Token metrics: input/output tokens and cost per thread.

#### Pane 4: Non-Blocking Input Composer (Bottom, fixed height)
* Independent input draft per thread: switching between threads preserves what you were typing.
* Automatic contextual reply prefix:
  `[Replying to: #### 2. Ingress Routing & DTOs]`
* **Zero-Friction Native Terminal Keybindings:**
  * `Enter`: Dispatch turn immediately (identical to native CLI agents).
  * `Shift+Enter` or `Ctrl+Enter`: Insert newline for multi-line drafting.
  * `Ctrl+E`: Open `$EDITOR` (Neovim/Helix/Nano) on a temporary scratch buffer, populating the input composer on save and exit.
* Shortcut bar showing available anchor switch keys (`Alt+1`, `Alt+2`, `Tab`).

---

### 3.2 Keyboard Navigation & Shortcuts

| Keybinding | Action |
| :--- | :--- |
| `Enter` | Dispatch message to active thread (in input composer). |
| `Shift+Enter` / `Ctrl+Enter` | Insert newline in input composer. |
| `Ctrl+E` | Open active prompt in `$EDITOR` (external editor). |
| `Tab` / `Shift+Tab` | Cycle focus between Panes (Threads $\to$ Transcript $\to$ Input $\to$ Blackboard). |
| `Alt+1` .. `Alt+9` | Instantly jump to and focus Anchor/Thread 1 through 9. |
| `Ctrl+N` | Fork new thread from currently selected message or heading anchor. |
| `Ctrl+R` | On-demand knowledge import: pull summarized conclusions from another thread. |
| `Ctrl+M` | Trigger thread merge (summarize branch decisions back to Trunk). |
| `Ctrl+B` | Toggle Blackboard pane visibility. |
| `o` | Open selected diff in external `$PAGER` (e.g., `delta`, `less`). |
| `Esc` | Return to Normal/Navigation mode from Input mode. |

---

### 3.3 Thread Status Lifecycle & Symbolic Indicators

To give the developer instant situational awareness across all active branches without switching panes, each entry in the Thread Navigator displays a compact status symbol and state marker:

| State | Unicode / Nerd Font | ASCII Fallback | Color Anchor | Semantic Meaning |
| :--- | :--- | :--- | :--- | :--- |
| **Streaming / Generating** | `⠋` (animated spinner) | `[*]` | Cyan / Yellow | Agent is actively generating tokens or reasoning thoughts. |
| **Executing Tool** | `⚙` / `▶` | `[>]` | Blue | Running a bash command, file read, or external process. |
| **Permission Request** | `⚠️` / `🔑` | `[!]` | Bold Amber | Blocked: requires user authorization (`Allow` / `Deny`). |
| **Awaiting Input** | `💬` / `?` | `[?]` | Magenta | Agent finished its turn; waiting for human prompt. |
| **Idle / Completed** | `✓` / `○` | `[-]` | Dim Green / Gray | Thread is quiescent with no pending turns. |
| **Error / Failed** | `✖` / `✗` | `[x]` | Red | Tool execution failed, command crashed, or rate limit hit. |
| **Dirty Workspace** | `*` (suffix) | `*` | Orange | Thread has uncommitted code modifications in its worktree. |
| **Unread Updates** | `●` (prefix) | `+` | Bright White | Completed turns or background events since last viewed. |

#### Visual Rendering Example (Pane 1):
```text
▼ Root: Architecture Design
  ├── [1] DB Schema *      ⚙ [cargo check]
  ├── [2] Ingress API      ⚠️ [needs permission]
  ├── [3] Frontend Runes   ● 💬 [waiting for you]
  └── [4] Auth Handlers    ✓ [-]
```

---

## 4. Technical Stack & Implementation Boundary

* **Language:** Rust (2021 edition)
* **TUI Layer:** `ratatui` + `crossterm`
* **Concurrency Runtime:** `tokio` (multi-threaded async runtime)
* **Local Persistence:** `rusqlite` or `redb` (embedded, zero external daemon dependency)
* **Serialization & RPC:** `serde`, `serde_json`, Unix Domain Sockets (`tokio::net::UnixListener`)
* **Markdown Parsing & Syntax Highlighting:** `pulldown-cmark`, `syntect`
* **Process Execution Model:** Native OS subprocess runner (`tokio::process::Command`) with `cwd` scoped to the dedicated worktree directory. Environment sanitization with explicit PATH pass-through. Zero container requirement (no Docker, no Podman).
* **Distribution Target:** Single static or self-contained binary packaged for Homebrew (`brew install`), Nix (`nix profile install` / Flake), and Cargo (`cargo install`).
* **Deployment Topology (Single Binary, Dual Mode):**
  * **Default (Standalone):** `grein` launches TUI and embeds the daemon engine internally within Tokio tasks. Zero socket setup required.
  * **Headless / Multi-Client:** `grein daemon` runs headless in the background; `grein attach` connects over a local Unix domain socket (`.grein/daemon.sock`).
  * **Headless Inspection Subcommands:** `grein threads`, `grein status`, and `grein agents --json` for scriptable and CLI introspection.

---

## 5. Verification & Specification Milestones

1. **Milestone 1:** Headless DAG Engine & Persistence (SQLite message DAG, branch forks, anchor parser).
2. **Milestone 2:** Daemon RPC Protocol (Unix socket server, streaming SSE events, tool execution loop).
3. **Milestone 3:** Ratatui Shell & Non-Blocking Multiplexer (Thread navigator, transcript viewer, composer).
4. **Milestone 4:** Lazy Worktree Allocator & Merging Engine (CoW worktrees, diff viewer, summary merge).