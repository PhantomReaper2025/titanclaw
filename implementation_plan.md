# IronClaw "Titan" Upgrade Plan: AGI & Orchestration

This upgraded plan outlines a fundamental architectural evolution to make IronClaw decisively **faster, more scalable, and significantly more powerful than OpenClaw**. Leveraging Rust's native concurrency, memory safety, and high-performance WebAssembly sandboxing, we will build a distributed, streaming, self-optimizing swarm intelligence.

## Goal Description

Transform IronClaw from a single-node, synchronous AI assistant into the **IronClaw "Titan" Engine**: a distributed, real-time, self-learning orchestration mesh. It must execute complex tasks orders of magnitude faster than OpenClaw by streaming data streams concurrently across thousands of lightweight micro-agents, bypassing LLM latency where possible, and continuously optimizing its own instructions.

## User Review Required

> [!CAUTION]
> This represents a paradigm shift from a linear conversational agent to a distributed orchestration mesh. Please confirm if this extreme scale of ambition aligns with your vision. 

## Implementation Status (2026-02-23)

| Track | Status | Notes |
|---|---|---|
| Swarm mesh runtime wiring | ✅ | `swarm` module is wired into runtime lifecycle behind config (`SWARM_ENABLED`, listen/heartbeat/max slots), incoming remote tasks execute through the local tool/safety stack, scheduler offload is capability-gated with deterministic local fallback, task assignments now include assignee-targeting (`assignee_node`) to prevent fan-out execution on every peer, receiving nodes suppress duplicate task IDs with a bounded TTL cache, and remote completion waiters are bounded with expiry cleanup. |
| Zero-latency text streaming | ✅ | Streaming chunk path is active in agent dispatcher to REPL/Web/WASM channels. |
| Tool/block-level streaming | ✅ | Tool start/completion/result events are live; shell tool streams incremental stdout/stderr, streamed tool-call deltas surface live shell command drafts (`[draft]`), piped early execution is default-on (toggle with `ENABLE_PIPED_TOOL_EXECUTION=false`), and approval-required commands emit explicit waiting status before execution. |
| Reflex compiler | ✅ | Background reflex compiler now persists normalized recurring patterns to a reflex registry and routes matching prompts directly to compiled tools before LLM fallback. |
| GraphRAG + AST indexing | ✅ | `memory_graph` now supports bounded multi-hop traversal, graph scoring, stable ranking, and semantic context fusion in responses. |
| Docker worker image provisioning hardening | ✅ | Orchestrator `job_manager` now preflights worker image availability and auto-pulls when missing (respects `sandbox.auto_pull_image`), eliminating first-run `No such image` container creation failures. |
| Chat lifecycle controls | ✅ | Web gateway now supports hard-delete for a single thread and clear-all chats (chat scope only), with UI actions. |
| Sandbox artifact export | ✅ | Web gateway supports direct archive download of sandbox project output (`/api/jobs/{id}/files/download`) and now streams tar output with a timeout watchdog instead of buffering the full archive in memory. |
| OpenCode bridge runtime | ✅ | `JobMode::OpenCode` now launches a dedicated `opencode-bridge` worker command (not generic `worker` fallback), passes model defaults through, and streams OpenCode output/events to orchestrator channels. |
| Routine FullJob scheduler integration | ✅ | `RoutineAction::FullJob` now creates a real job context, schedules it through `Scheduler`, and reports success/failure from terminal job state instead of lightweight fallback. |
| Anticipatory execution (shadow workers) | ✅ | New `shadow_engine` precomputes likely next-turn responses via bounded cheap-LLM workers, caches by normalized prompt, and serves cache hits instantly with TTL + background pruning. |
| Self-modifying kernel orchestration | ✅ | New `kernel_orchestrator` loop records live tool latencies from dispatcher execution, detects bottlenecks, creates patch proposals in `KernelMonitor`, and can auto-approve/auto-deploy via `jit_wasm_run` (config-gated). User-facing control is live via `kernel_patch` tool, `/kernel ...` commands, and gateway `/api/kernel/patches*` endpoints. |
| Reliability hardening (runtime + onboarding) | ✅ | Fixed scheduler subtask success tracking, removed sandbox job-manager panic path, added OAuth callback `state` verification/consumption for WASM auth callbacks, enabled WASM channel setup validation endpoint GET checks with secret placeholder substitution, improved medium-path logging/error surfacing (worker progress status, self-repair broadcasts, scheduler start signal, container cleanup), hardened the web gateway path (URL-decoded SSE auth query tokens, parsed loopback WebSocket Origin validation, DOM-based chat markdown sanitization), hardened sandbox completion handling with structured `/worker/{job_id}/complete` termination plus broader fallback phrase detection/anti-loop handling, made swarm remote wait fallback timeout configurable (`SWARM_REMOTE_WAIT_TIMEOUT_MS`, clamped), and added restartable supervisors (with backoff + shutdown signaling) for critical agent background loops (self-repair, kernel monitor, shadow prune, reflex compiler). |
| WASM tool metadata/schema registration | ✅ | WASM metadata now resolves via loader sidecar overrides first, then true WIT export introspection (`description()` / `schema()`) during runtime preparation, with explicit warning-logged fallback metadata only as last resort. |
| Dynamic profile synthesis (managed identity docs) | ✅ | New background profile synthesizer batches successful turns asynchronously (debounced), extracts durable preferences/facts, and updates managed sections in `AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, and `MEMORY.md` while preserving manual content outside markers. |
| Conversational profile onboarding (OpenClaw-style) | ✅ | Added a shared agent-layer first-chat onboarding flow (soft-block) across web/CLI/Telegram that captures user identity, goals, tone, execution style, and boundaries, supports `/onboard profile` commands, and writes reviewed managed baseline sections into `AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, and `MEMORY.md`. |
| Session/thread workflow hardening (races + approvals) | ✅ | `resolve_thread` now serializes mapping creation and preserves UUID external thread IDs for the gateway/web UUID-backed thread flow (avoiding duplicate-thread/hydration mismatches) while keeping non-gateway channels channel-scoped, `register_thread` warns on collisions, hydration avoids overwriting concurrently created threads, thread delete/clear now clean in-memory thread mappings/undo state immediately, and pending approvals now expire automatically instead of waiting forever. |
| Chat durability + turn-state hardening | ✅ | Turn persistence now retries DB writes and emits a visible warning on final failure, runtime thread paths use guarded `try_start_turn()` to prevent invalid state transitions, and approval resume/rejection flows now close/persist turns consistently instead of leaving partial state. |
| Embeddings input length guard semantics | ✅ | Embedding providers now validate approximate character length (not byte length), apply checks consistently to both single and batch embedding calls, and return clearer `TextTooLong` values for non-ASCII inputs. |
| WhatsApp unsupported media handling | ✅ | Non-text WhatsApp messages are no longer silently dropped; the channel emits an explicit placeholder message to the agent and logs a warning so users can be informed to resend as text. |
| PostgreSQL AST graph query gap (documented) | ✅ | AST graph query remains Database/libSQL-only; runtime error message and docs now explicitly state the PostgreSQL-backed workspace limitation. |
| Autonomy Control Plane v1 groundwork (types + persistence schema) | 🚧 | Added versioned autonomy domain types (`src/agent/autonomy.rs`), implemented Postgres/libSQL autonomy store CRUD for goals/plans/steps/execution/policy/verification records plus plan-step read helpers and atomic plan-step replacement, added schema migrations/scaffolding (`V11`-`V18` + libSQL mirror), wired best-effort worker/dispatcher persistence so planned worker runs and chat tool approvals/execution attempts now emit DB-backed autonomy records (including worker post-plan verifications with richer per-step checks/evidence and early completion-path coverage for persisted plans), persisted `agent_jobs` autonomy link fields for restart-safe correlation, exposed user-scoped gateway goal/plan create + inspection + status update/reprioritization APIs (including list filtering/sorting/pagination via `status`/`sort`/`offset`/`limit` on top-level and goal-scoped plan lists plus convenience lifecycle aliases including explicit `cancel` endpoints), plan-step list/create/detail/status/replace APIs, plan revisioning (`POST /api/plans/{id}/replan`, with optional source-step copying or inline step payloads), and telemetry inspection APIs (`GET /api/plans/{id}/executions`, `GET /api/plans/{id}/verifications` with `status`/`sort`/`offset`/`limit`, `GET /api/goals/{id}/policy-decisions`), and added CLI `goal`/`plan`/`plan-step` create-list-show-set-status/set-priority plus list filters (`goal/plan list --status --sort --offset --limit`), `plan verifications --status --sort --offset --limit`, lifecycle aliases including `goal cancel` / `plan cancel`, `plan replan --copy-steps|--steps-file|--steps-json`, and `plan-step replace` commands for local inspection/manual management. |

## Execution TODO (Live)

- [x] Zero-latency text streaming to channels
- [x] Shell tool incremental output streaming
- [x] Deterministic NL reflex fast-path for job intents
- [x] AST graph indexing + symbol-level query tool (`memory_graph`)
- [x] Generalized reflex routing from recurring patterns to compiled tools
- [x] Multi-hop GraphRAG retrieval quality hardening
- [x] Token-to-tool piped execution completion (default-on with explicit approval-aware piped status)
- [x] Swarm workload distribution from scheduler into mesh peers (scheduler tool subtasks offload to swarm with fast local fallback)
- [x] Docker job runner preflight image check + auto-pull fallback for first-run reliability
- [x] Shadow-worker speculative execution cache with bounded concurrency + TTL
- [x] Kernel monitor runtime loop with proposal generation + optional auto deploy
- [x] OAuth callback state nonce verification for WASM extension auth callbacks
- [x] WASM channel setup validation endpoint execution (GET with secret placeholder substitution)
- [x] Medium reliability hardening for silent-failure logging + unsupported channel message types
- [x] True WASM/WIT tool metadata introspection (`description()` / `schema()`) with safe fallback
- [x] Async debounced profile synthesis into managed sections of core identity docs
- [x] Conversational profile onboarding in chat with review+confirm and managed-doc baseline write
- [x] Autonomy Control Plane v1 groundwork: domain types + DB trait surface + Postgres/libSQL schema scaffolding (`V11`-`V16`)
- [x] Autonomy Control Plane v1 runtime persistence instrumentation (worker plan bridge + dispatcher policy/execution records, best-effort)
- [x] Persist job-level autonomy linkage IDs in `agent_jobs` across Postgres/libSQL (`V17` + libSQL schema compatibility path)
- [x] Read-only gateway inspection endpoints for autonomy goals/plans (user-scoped)
- [x] Gateway create endpoints for autonomy goals/plans (`POST /api/goals`, `POST /api/plans`, user-scoped)
- [x] Gateway goal/plan status update endpoints (`POST /api/goals/{id}/status`, `POST /api/plans/{id}/status`, user-scoped)
- [x] Gateway goal/plan lifecycle alias endpoints (cancel/complete/abandon/supersede convenience actions, user-scoped)
- [x] Gateway plan-step endpoints (list/create/detail/status) with user-scoped ownership checks
- [x] Gateway atomic plan-step replace endpoint for replans (`POST /api/plans/{id}/steps/replace`)
- [x] Gateway plan replan endpoint (`POST /api/plans/{id}/replan`) for next-revision creation with optional supersede and optional source-step copy or inline step payload
- [x] Gateway telemetry inspection endpoints for autonomy execution attempts / policy decisions (user-scoped)
- [x] Gateway plan verification inspection endpoint (`GET /api/plans/{id}/verifications`) plus worker post-plan verification record persistence (best-effort, user-scoped inspection)
- [x] CLI plan verification inspection command (`titanclaw plan verifications`) with user-scoped ownership validation and optional limit
- [x] Goal-scoped plan list and plan-verification inspection query ergonomics (`status`/`sort`/`offset`/`limit` in web; CLI `plan verifications --status --sort --offset --limit`)
- [x] CLI autonomy goal/plan commands (`goal|plan create/list/show`) with DB-backed persistence access
- [x] CLI autonomy goal/plan status updates (`goal set-status`, `plan set-status`) with user-scoped ownership validation
- [x] CLI goal reprioritize command (`goal set-priority`) with user-scoped ownership validation
- [x] Goal/plan list filtering/sorting/pagination (`status`, `sort`, `offset`, `limit`) in web query params and CLI list commands
- [x] CLI goal/plan lifecycle alias commands (`goal cancel|complete|abandon`, `plan cancel|complete|supersede`)
- [x] CLI plan-step commands (`plan-step create/list/show/set-status`) with user-scoped ownership validation via plan->goal
- [x] CLI plan-step bulk replace (`plan-step replace`) with atomic DB-backed step replacement
- [x] CLI plan replan command (`plan replan`) for next-revision creation with optional supersede and optional `--copy-steps` or `--steps-file/--steps-json`

## Proposed Changes

### 1. The "Hive" Distributed Swarm Architecture
*OpenClaw is restricted to Node.js's single-threaded event loop and monolithic deployments. We will shatter this bottleneck.*
*   **Peer-to-Peer Agent Mesh**: Integrate a gossip protocol (e.g., using `libp2p` or Turso/libSQL edge sync) allowing multiple IronClaw instances (laptop, desktop, phone) to cluster together.
*   **Workload Distribution**: The orchestration scheduler ([src/agent/scheduler.rs](file:///home/phantom/Documents/TitanClaw/titanclaw/src/agent/scheduler.rs)) will offload heavy operations (like semantic searches, WASM sandbox execution, or local LLM inference) across the mesh to utilize all available hardware.
*   **Massive Concurrency**: Spawn thousands of specialized Tokio tasks representing micro-agents that can crawl the web, search workspace memory, and analyze code *simultaneously* rather than sequentially.

### 2. Zero-Latency Streaming State Machine
*Eliminate the "waiting for LLM" feeling.*
*   **Piped Execution (Z.AI Parity & Beyond)**: Implement true tool and block-level streaming. If the primary reasoning agent decides to write a script, the `shell` tool begins executing the script *as it is being streamed* from the LLM, passing partial output to a validation sub-agent instantly.
*   **Canvas & Live TUI**: Implement agent-driven UI (Canvas) over WebSockets/SSE and Ratatui. Users watch the agent's thought process, tool execution, and code generation materialize simultaneously in real-time.

### 3. "Reflex" Layer & Continuous Learning (Fast Path)
*True AGI doesn't spend 5 seconds thinking about a problem it solved yesterday.*
*   **WASM Micro-Skills compiling**: The background `routine_engine.rs` (Reflection) analyzes common job graphs and LLM responses. It will use the dynamic builder (`tools/builder/`) to compile highly-optimized Rust/WASM "Reflexes".
*   **Bypassing the LLM**: When the router detects a known intent (e.g., "summarize my daily log"), it triggers the sub-millisecond WASM reflex instead of invoking the LLM provider, making recurring tasks instant.

### 4. Fluid Memory & GraphRAG
*Move beyond OpenClaw's static chunk-based RRF search.*
*   **GraphRAG Generation**: As the agent reads files and converses, background agents continuously construct a Knowledge Graph (entities, relationships, concepts) within the PostgreSQL/libSQL database.
*   **Implicit Context Injection**: Instead of explicitly querying memory, the system uses "presence" and context awareness. When you open a specific code file, IronClaw pre-fetches the entire historical context of who wrote it, why, and related bugs sub-millisecond.

### 5. Total Provider Independence (Zero-Bottleneck inference)
*A truly massive swarm cannot route its traffic through a single web proxy.*
*   **Direct API Connections**: Rip out the hard dependency on the `NEAR AI` proxy router in [src/llm/nearai.rs](file:///home/phantom/Documents/TitanClaw/titanclaw/src/llm/nearai.rs). The mesh will connect directly to Anthropic, OpenAI, or local endpoints.
*   **Local Dominance**: Default thousands of micro-worker tasks to high-speed local inference (via Ollama or vLLM bindings) to completely eliminate network TTFB and API costs for repetitive/simple tasks.
*   **Decentralized Onboarding**: Rewrite the `ironclaw onboard` CLI to offer a totally offline, proxy-free setup path so the engine can run completely air-gapped if desired.

### 6. Anticipatory Execution (The "Pre-Cog" Engine)
*Waiting for an AI to act after you ask is too slow. The engine must act before you ask.*
*   **Shadow Workers**: While you are reading a webpage or typing a document, background agents continuously read your active screen state context. They predict what you need next (e.g., formatting data, writing a test for the code you just typed).
*   **Speculative Execution**: The engine preemptively runs the LLM and tool calls in hidden sandbox environments for its top 3 predictions of what you will ask next. When you finally ask, the answer is already computed and materializes with 0ms latency. 

### 7. Self-Modifying WASM Kernel
*True AGI must be able to upgrade its own source code securely at runtime.*
*   **Deep Reflection**: A dedicated background agent constantly monitors the engine's own logs/errors in [src/agent/agent_loop.rs](file:///home/phantom/Documents/TitanClaw/titanclaw/src/agent/agent_loop.rs) and performance bottlenecks.
*   **Runtime Patching**: When it detects an inefficiency in how a tool (like `memory_search`) performs, it writes a faster version in Rust, compiles it to WASM, and hot-swaps the capability instantly, making the core engine progressively faster the longer it runs.

### 8. Context-Aware Just-In-Time (JIT) Tool Compilation
*Don't rely on pre-built generic tools. Build the perfect tool for the micro-second it's needed.*
*   **Disposable Scripts**: Instead of trying to force a generic `bash` or `python` tool to scrape a complex, heavily-obfuscated website, the agent dynamically compiles a specialized Rust/WASM scraping binary tailored *exactly* to that one website's DOM structure in under 500ms, executes it for max speed, and then destroys it.

### 9. Introspective Semantic Indexing (Deep Context RAG)
*Pushing the existing Workspace system to its absolute limits safely.*
*   **Codebase Syntax AST Indexing**: Instead of just text chunks, the background agents use tree-sitter to parse your entire workspace into an Abstract Syntax Tree (AST), understanding relationships (e.g., "Function A calls Function B"). This integrates perfectly with the existing `src/workspace/` module.
*   **Latent Space Tagging**: When files are saved, a fast local model automatically assigns semantic tags and summaries to the metadata database tables, making the existing RRF search incredibly precise.

### 10. Local Docker Sandbox Swarming
*Leveraging the existing Orchestrator (`src/orchestrator/`) to its full potential.*
*   **Swarm Execution**: The current system runs one Claude Code container per job. The Titan engine will use the existing `job_manager.rs` to spawn *dozens* of ephemeral, specialized Docker sandboxes in parallel.
*   **Distributed Code Gen**: If you ask for a full-stack app, it spins up three isolated sandboxes: one writes the frontend, one writes the backend, and one writes the database schema, compiling everything simultaneously and safely natively merging the results.

### 11. Infinite Self-Expansion & Hot-Swapping
*The system must never need a "restart" to learn a new paradigm.*
*   **Dynamic Channel & Provider Generation**: Enhance dynamic tool building to allow IronClaw to write its own Channel adapters (e.g., a new Discord integration) and Database backends.
*   **Zero-Downtime Hot-Swapping**: It will compile these as WASM modules or loadable dynamic libraries and inject them into `ChannelManager` while the agent is running. 

## Phased Rollout Strategy (Safe Execution)

To prevent the massive complexity of this plan from breaking the existing, stable IronClaw codebase, we will implement this iteratively, starting with features that build directly on what exists today:

### Phase 0: Make it feel magical immediately (Current Focus)
*Builds safely on existing `tools/builder` and `channels/web` without rewriting core routing.*
*   **Reflex Engine + WASM Skill Compiler**: Automatically compile repeated LLM prompts into fast WASM tools.
*   **Zero-Latency Streaming**: Pipe streaming output directly to the existing Ratatui TUI and Web GUI.
*   **Local Ollama Dominance**: Set the default orchestrator to route basic tasks to local open-source models using the existing `rig::providers::ollama` implementation.

### Phase 1: Core Superpowers
*   **GraphRAG + Tree-Sitter AST**: Safely run alongside the existing FTS SQLite database before replacing it.
*   **JIT Disposable WASM Tools**: Compile and run single-use tools safely in the existing `tools/wasm/` sandbox.

### Phase 2: The Swarm & Self-Modification
*   **Full libp2p Hive**: Transition from single-node `tokio` to multi-node distributed mesh.
*   **Self-Modifying Kernel**: Introduce runtime patching with a strict, sandboxed human-veto approval queue.

## Recommended Tech Stack Update

We will lock in these specific, production-ready Rust crates for the Titan Engine to ensure memory safety and concurrency:
*   **Swarm Mesh**: `libp2p` (gossipsub, Kademlia) + `quinn` (QUIC transport) + `tokio`.
*   **WASM Runtime**: Keep Wasmtime, heavily utilize `wasmtime-wasi` and the Component Model (`wit-bindgen`) for strict hot-swapping contracts.
*   **GraphRAG**: `graphrag-rs` (for HNSW and PageRank logic) and `tree-sitter` for real-time syntax indexing.

## Verification Plan

### Automated Tests
*   **Mesh Concurrency Benchmarks**: Add tests in `benchmarks/` to ensure the distributed scheduler can handle 10,000+ simultaneous mock tasks across multiple worker threads and isolated WASM sandboxes.
*   **Streaming Latency Tests**: Validate that the Time-To-First-Byte (TTFB) of tool execution output begins within 100ms of the LLM provider streaming the tool invocation token.

### Manual Verification
1.  **Swarm Test**: Ask the Titan engine to "Review the entire Linux kernel commit history for the past week, summarize security patches, and draft a blog post." Observe it spawning hundreds of concurrent worker threads.
2.  **Continuous Learning Test**: Ask it the same complex logic puzzle twice. The first time should take 10 seconds (LLM). The system background compiles a reflex. The second time should take <100ms (WASM fast path).
