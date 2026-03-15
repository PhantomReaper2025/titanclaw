# GEMINI.md - TitanClaw Project Context

## Project Overview
**TitanClaw** (successor to IronClaw) is a security-first, distributed AI orchestration mesh built in Rust. It is designed for high-throughput, secure tool execution across multi-provider LLMs, utilizing a combination of local-first memory, WebAssembly (WASM) sandboxing, and Docker-isolated workers.

### Core Architecture
- **Distributed Hive Mesh:** Peer-to-peer agent mesh using `libp2p` for workload distribution.
- **Autonomy Control Plane (v1):** Orchestrates tasks using a structured Hierarchy: Goals -> Plans -> Steps.
- **Memory Plane (v2):** A structured memory system with Episodic, Semantic, and Procedural memory promotion via a supervised consolidation loop.
- **Tooling System (v2):** Features "Tool Contract V2" for side-effect safety, reliability profiles, and circuit-breaker routing.
- **Reflex Layer:** A compiler that turns recurring natural language intents into optimized WASM "Reflex" skills to bypass LLM latency.
- **Security & Sandboxing:** Multi-layered isolation using WASM (wasmtime) for lightweight tools and Docker for heavy-duty sandbox jobs.
- **Streaming State Machine:** Supports tool and block-level streaming, including real-time shell output and piped execution.

## Tech Stack
- **Language:** Rust (Edition 2024, v1.92+)
- **Async Runtime:** Tokio
- **API/Gateway:** Axum (REST, SSE, WebSocket)
- **Databases:** PostgreSQL (primary, with `pgvector`) and libSQL (embedded/Turso sync).
- **LLM Integration:** Multi-provider support via `rig-core`.
- **Infrastructure:** Docker (for workers), `libp2p` (for swarm mesh), Wasmtime (for tool sandbox).

## Key Commands

### Development
- **Build:** `cargo build --release`
- **Test:** `cargo test`
- **Lint:** `cargo clippy --all --all-features`
- **Format:** `cargo fmt`

### Runtime Operations
- **Onboard (Wizard):** `./target/release/titanclaw onboard`
- **Run (Start Agent):** `./target/release/titanclaw run`
- **Status/Health:** `./target/release/titanclaw status` or `./target/release/titanclaw doctor`
- **Manage Tools:** `./target/release/titanclaw tool --help`
- **Manage Memory:** `./target/release/titanclaw memory --help`

## Development Conventions

### Documentation Policy
Five canonical files must be kept in sync when behavior changes:
1. `AGENTS.md`
2. `IDENTITY.md`
3. `README.md`
4. `implementation_plan.md`
5. `CHANGELOG.md`

### Managed Content
The project uses marker-delimited blocks (e.g., `<!-- titan_managed_start -->`) in core identity docs. The `ProfileSynthesizer` auto-updates these sections; manual content outside these markers is preserved.

### Coding Standards
- **Imports:** Use `crate::` instead of `super::`.
- **Errors:** Use `thiserror` for library errors and `anyhow` for CLI/app-level errors.
- **Panics:** Strictly no `.unwrap()` or `.expect()` in production code (enforced by project policy).
- **Persistence:** All new features must implement the `Database` trait and support both PostgreSQL and libSQL backends.
- **Tool Logic:** Keep service-specific logic out of the core agent. Use `capabilities.json` for tool requirements.

### Tool Types
- **Built-in:** Compiled directly into the Rust binary.
- **WASM:** Preferred for new capabilities (sandboxed, metered, secure).
- **MCP:** Supported for external process-based tool servers.

## Directory Structure Highlights
- `src/agent/`: Core orchestration logic (Autonomy, Memory Plane, Reflex, Scheduler).
- `src/channels/`: Input/Output handlers (Web Gateway, CLI/TUI, WASM channels).
- `src/tools/`: Extensible tool system (WASM runtime, MCP client, Contract V2).
- `src/orchestrator/`: Manages Docker worker lifecycles.
- `src/worker/`: The agent code that runs inside isolated Docker containers.
- `src/workspace/`: Persistent memory and GraphRAG (AST indexing) logic.
- `migrations/`: SQL schema migrations for PostgreSQL.

## Current Focus (2026-03-05)
- **Autonomy Control Plane v1:** Supervised worker/chat runtime paths are live.
- **Memory Plane v2:** Consolidation and retrieval composer are active.
- **Tooling System v2:** Phase 3 reliability profiles and proactive rerouting are being finalized.
- **Swarm Mesh:** Experimental distributed mesh via `libp2p` is wired.
