# IronClaw "Titan" Upgrade Plan: AGI & Orchestration

This upgraded plan outlines a fundamental architectural evolution to make IronClaw decisively **faster, more scalable, and significantly more powerful than OpenClaw**. Leveraging Rust's native concurrency, memory safety, and high-performance WebAssembly sandboxing, we will build a distributed, streaming, self-optimizing swarm intelligence.

## Goal Description

Transform IronClaw from a single-node, synchronous AI assistant into the **IronClaw "Titan" Engine**: a distributed, real-time, self-learning orchestration mesh. It must execute complex tasks orders of magnitude faster than OpenClaw by streaming data streams concurrently across thousands of lightweight micro-agents, bypassing LLM latency where possible, and continuously optimizing its own instructions.

## User Review Required

> [!CAUTION]
> This represents a paradigm shift from a linear conversational agent to a distributed orchestration mesh. Please confirm if this extreme scale of ambition aligns with your vision. 

## Implementation Status (2026-02-20)

| Track | Status | Notes |
|---|---|---|
| Swarm mesh runtime wiring | 🚧 | `swarm` module exists and is now wired into main runtime lifecycle behind config (`SWARM_ENABLED`, listen/heartbeat/max slots). |
| Zero-latency text streaming | ✅ | Streaming chunk path is active in agent dispatcher to REPL/Web/WASM channels. |
| Tool/block-level streaming | 🚧 | Tool start/completion/result events are live; shell tool streams incremental stdout/stderr, and streamed tool-call deltas now surface live shell command drafts (`[draft]`) before final tool-call resolution. Full execute-while-generating behavior remains partial. |
| Reflex compiler | 🚧 | Background reflex compiler loop is running; deterministic natural-language fast-path now bypasses LLM for high-confidence job intents (`list/status/cancel/help/create`), but generalized reflex routing is still incomplete. |
| GraphRAG + AST indexing | 🚧 | Tree-sitter AST graph indexing is integrated and queryable through `memory_graph` with bounded multi-hop traversal, graph scoring, and semantic context fusion; higher-order reasoning quality tuning is still in progress. |

## Execution TODO (Live)

- [x] Zero-latency text streaming to channels
- [x] Shell tool incremental output streaming
- [x] Deterministic NL reflex fast-path for job intents
- [x] AST graph indexing + symbol-level query tool (`memory_graph`)
- [ ] Token-to-tool piped execution completion (live tool-call draft piping is live; full execute-before-final-resolution path still pending)
- [ ] Generalized reflex routing from recurring patterns to compiled tools
- [ ] Multi-hop GraphRAG retrieval quality hardening (cross-file traversal + initial ranking/context fusion are live; advanced reasoning quality tuning remains)
- [ ] Swarm workload distribution from scheduler into mesh peers

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
