# TitanClaw Comprehensive Architecture Analysis

## Executive Summary

TitanClaw is a sophisticated Rust-based AI agent system designed for autonomous operation with strong safety guarantees. The architecture demonstrates a **multi-layered approach to autonomy** with several advanced mechanisms including self-repair, autonomous routines, reflexive behavior compilation, and kernel monitoring. The system is built around a central agent loop that coordinates multiple subsystems through well-defined interfaces.

### Key Architectural Highlights

1. **Robust Agent Core** - Event-driven architecture with supervised background tasks
2. **WASM-Based Tool System** - Secure, sandboxed tool execution with capability-based permissions
3. **Multi-Channel Communication** - Support for Discord, Slack, Telegram, WhatsApp, and Web Gateway
4. **Autonomous Routines** - Cron-based and event-triggered automated behaviors
5. **Self-Repair Mechanisms** - Automatic detection and recovery from stuck jobs and broken tools
6. **Profile Synthesis** - Dynamic learning from conversations to update identity documents

---

## 1. Core Agent Architecture

### 1.1 Agent Loop ([`src/agent/agent_loop.rs`](src/agent/agent_loop.rs))

The main agent loop is the heart of TitanClaw, implementing an event-driven architecture with multiple background supervisors.

#### Key Components

**[`AgentDeps`](src/agent/agent_loop.rs:143) struct** - Core dependencies bundle:
```rust
pub struct AgentDeps {
    pub store: Option<Arc<dyn Database>>,
    pub llm: Arc<dyn LlmProvider>,
    pub cheap_llm: Option<Arc<dyn LlmProvider>>,  // Fast LLM for routing/heartbeat
    pub safety: Arc<SafetyLayer>,
    pub tools: Arc<ToolRegistry>,
    pub workspace: Option<Arc<Workspace>>,
    pub extension_manager: Option<Arc<ExtensionManager>>,
    pub skill_registry: Option<Arc<std::sync::RwLock<SkillRegistry>>>,
    pub hooks: Arc<HookRegistry>,
    pub cost_guard: Arc<crate::agent::cost_guard::CostGuard>,
    pub swarm_handle: Option<SwarmHandle>,         // Distributed task offload
    pub swarm_results: Option<SwarmResultRouter>,
    pub container_job_manager: Option<Arc<ContainerJobManager>>,
    pub kernel_orchestrator: Option<Arc<KernelOrchestrator>>,
    pub shadow_engine: Option<Arc<ShadowEngine>>,  // Speculative execution
}
```

**[`Agent`](src/agent/agent_loop.rs:173) struct** - Main coordinator:
- `context_manager` - Tracks active job contexts
- `scheduler` - Task scheduling and execution
- `router` - Message intent routing
- `session_manager` - User session handling
- `context_monitor` - Token usage tracking
- `profile_onboarding` - Conversational profile setup
- `profile_synthesizer` - Dynamic profile updates

#### Main Loop Flow ([`run()`](src/agent/agent_loop.rs:360))

```
┌─────────────────────────────────────────────────────────────┐
│                    AGENT STARTUP                            │
├─────────────────────────────────────────────────────────────┤
│  1. Start all channels (Discord, Slack, Telegram, etc.)    │
│  2. Create shutdown signal for background supervisors       │
│  3. Spawn supervised background tasks:                      │
│     - Self-repair task (stuck jobs, broken tools)          │
│     - Kernel orchestrator (performance monitoring)          │
│     - Shadow prune (speculative execution cleanup)          │
│     - Session pruning (stale session cleanup)               │
│     - Heartbeat (proactive check-ins)                       │
│     - Routine engine (cron + event triggers)                │
│     - Reflex compiler (recurring pattern → WASM tools)      │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    MAIN MESSAGE LOOP                        │
├─────────────────────────────────────────────────────────────┤
│  tokio::select! {                                           │
│    ctrl_c → shutdown                                        │
│    message → handle_message()                               │
│  }                                                          │
│                                                             │
│  handle_message() flow:                                     │
│  1. Parse submission type (UserInput, Command, etc.)        │
│  2. Run BeforeInbound hooks (modify/reject)                 │
│  3. Hydrate thread from DB if needed                        │
│  4. Resolve session and thread                              │
│  5. Check for pending auth mode interception                │
│  6. Check for profile onboarding interception               │
│  7. Process based on submission type                        │
│  8. Run AfterOutbound hooks                                 │
│  9. Check event triggers for routines                       │
└─────────────────────────────────────────────────────────────┘
```

#### Supervised Background Tasks ([`spawn_supervised_background_task()`](src/agent/agent_loop.rs:64))

A sophisticated supervision pattern with:
- **Exponential backoff** on restart (1s → 30s max)
- **Graceful shutdown** via watch channel
- **Panic recovery** with logging
- **Restart counting** for diagnostics

### 1.2 Dispatcher ([`src/agent/dispatcher.rs`](src/agent/dispatcher.rs))

The dispatcher implements the **agentic loop** - the core LLM → Tool → Response cycle.

#### Agentic Loop ([`run_agentic_loop()`](src/agent/dispatcher.rs:162))

```
┌─────────────────────────────────────────────────────────────┐
│                  AGENTIC LOOP                               │
├─────────────────────────────────────────────────────────────┤
│  MAX_TOOL_ITERATIONS = 10                                   │
│                                                             │
│  loop {                                                     │
│    1. Check for interruption                                │
│    2. Enforce cost guardrails                               │
│    3. Refresh tool definitions (hot-reload support)         │
│    4. Apply skill-based tool attenuation                    │
│    5. Call LLM with streaming                               │
│       - On text chunk → StreamChunk status update           │
│       - On tool call → Early piped execution for shell      │
│    6. Record cost and token usage                           │
│    7. Match result:                                         │
│       - Text → Return if tools executed, else prompt again  │
│       - ToolCalls → Execute each tool:                      │
│         a. Check approval requirements                      │
│         b. Run BeforeToolCall hooks                         │
│         c. Execute with timeout                             │
│         d. Sanitize output                                  │
│         e. Check for auth awaiting mode                     │
│         f. Add tool result to context                       │
│  }                                                          │
└─────────────────────────────────────────────────────────────┘
```

#### Piped Tool Execution ([lines 361-476](src/agent/dispatcher.rs:361))

An optimization for shell commands:
- **Early start** - Begin execution while LLM is still streaming
- **Draft preview** - Show command being built in real-time
- **Result caching** - Store early results for instant retrieval

### 1.3 Session Management

#### Session ([`src/agent/session.rs`](src/agent/session.rs))

**[`Session`](src/agent/session.rs) struct** - User session container:
- `threads` - Map of thread ID to Thread
- `auto_approved_tools` - Tools that don't require approval

**[`Thread`](src/agent/session.rs) struct** - Conversation thread:
- `turns` - History of conversation turns
- `state` - ThreadState (Idle, Processing, Interrupted, AwaitingApproval)
- `pending_approval` - Tool awaiting user approval
- `pending_auth` - Extension awaiting auth token

**[`Turn`](src/agent/session.rs) struct** - Single conversation turn:
- `user_input`, `response` - Content
- `tool_calls` - Tool invocations with results
- `state` - TurnState (InProgress, Completed, Failed)

#### Session Manager ([`src/agent/session_manager.rs`](src/agent/session_manager.rs))

**[`SessionManager`](src/agent/session_manager.rs) struct**:
- `sessions` - User ID → Session mapping
- `thread_map` - ThreadKey → UUID mapping (preserves external thread IDs)
- `undo_managers` - Per-thread undo history

**Key method [`resolve_thread()`](src/agent/session_manager.rs)**:
- Maps external thread IDs to internal UUIDs
- Preserves gateway/web UUID to avoid hydration mismatches
- Creates new threads on demand

### 1.4 Router ([`src/agent/router.rs`](src/agent/router.rs))

Simple but effective message routing:

**[`MessageIntent`](src/agent/router.rs) enum**:
- Fast-path natural language detection for common commands
- Command parsing with `/` prefix
- Delegation to appropriate handlers

---

## 2. Memory Systems

### 2.1 Workspace Memory ([`MEMORY.md`](MEMORY.md))

A simple but effective long-lived memory system:
- **Stable Facts** - Product name, repository, default goals
- **Operational Decisions** - Architecture decisions, preferences
- **Follow-up Rules** - Memory maintenance guidelines

### 2.2 Memory Handlers ([`src/channels/web/handlers/memory.rs`](src/channels/web/handlers/memory.rs))

REST API for workspace operations:
- `GET /api/memory/tree` - File tree listing
- `GET /api/memory/list` - Directory contents
- `GET /api/memory/read` - File content
- `POST /api/memory/write` - Write file
- `POST /api/memory/search` - Search workspace

### 2.3 Context Compaction ([`src/agent/compaction.rs`](src/agent/compaction.rs))

**[`ContextCompactor`](src/agent/compaction.rs:34)** manages context window limits:

**Strategies**:
1. **Summarize** - LLM-generated summary of old turns, written to daily log
2. **Truncate** - Simple removal of old turns
3. **MoveToWorkspace** - Archive full context to workspace files

**Compaction flow**:
```
Thread → Analyze tokens → Select strategy → Execute → Write summary to daily log
```

---

## 3. Tool Integration (WASM System)

### 3.1 Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    WASM TOOL SYSTEM                         │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐     │
│  │   Loader    │───▶│   Runtime   │───▶│    Host     │     │
│  │             │    │             │    │             │     │
│  │ Discover    │    │ Compile     │    │ State       │     │
│  │ Load        │    │ Cache       │    │ Capabilities│     │
│  │ Validate    │    │ Execute     │    │ Rate Limit  │     │
│  └─────────────┘    └─────────────┘    └─────────────┘     │
│         │                  │                  │             │
│         ▼                  ▼                  ▼             │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐     │
│  │   Schema    │    │   Router    │    │  Wrapper    │     │
│  │             │    │             │    │             │     │
│  │ Capabilities│    │ HTTP routes │    │ WIT bindings│     │
│  │ JSON config │    │ Webhooks    │    │ Imports     │     │
│  └─────────────┘    └─────────────┘    └─────────────┘     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Loader ([`src/channels/wasm/loader.rs`](src/channels/wasm/loader.rs))

**[`WasmChannelLoader`](src/channels/wasm/loader.rs)**:
- Discovers `.wasm` files with `.capabilities.json` sidecars
- Validates channel structure
- Returns `LoadResults` with success/error separation

### 3.3 Runtime ([`src/channels/wasm/runtime.rs`](src/channels/wasm/runtime.rs))

**[`WasmChannelRuntime`](src/channels/wasm/runtime.rs)**:
- Engine configuration with fuel limits
- Compiled module caching
- Per-module limits and timeouts

**[`WasmChannelRuntimeConfig`](src/channels/wasm/runtime.rs)**:
- `default_limits` - Memory, CPU constraints
- `fuel_config` - Instruction counting
- `cache_compiled` - AOT compilation
- `callback_timeout` - Execution deadlines

### 3.4 Host ([`src/channels/wasm/host.rs`](src/channels/wasm/host.rs))

**[`ChannelHostState`](src/channels/wasm/host.rs)** - Runtime state for WASM execution:
- `emitted_messages` - Messages to send
- `pending_writes` - Workspace writes
- `emit_count` / `emits_dropped` - Rate limiting

**Security features**:
- `MAX_EMITS_PER_EXECUTION` - Prevent message spam
- `MAX_MESSAGE_CONTENT_SIZE` - Limit message size
- Path traversal protection for workspace writes
- HTTP allowlist checking

### 3.5 Schema ([`src/channels/wasm/schema.rs`](src/channels/wasm/schema.rs))

**[`ChannelCapabilitiesFile`](src/channels/wasm/schema.rs)** - JSON configuration:
```json
{
  "type": "wasm-tool",
  "name": "telegram",
  "description": "Telegram Bot integration",
  "setup": {
    "required_secrets": [{"name": "token", "prompt": "Bot token"}]
  },
  "capabilities": {
    "tool": { ... },
    "channel": {
      "allowed_paths": ["/telegram"],
      "emit_rate_limit": { "messages_per_minute": 20 }
    }
  }
}
```

### 3.6 Router ([`src/channels/wasm/router.rs`](src/channels/wasm/router.rs))

**[`WasmChannelRouter`](src/channels/wasm/router.rs)**:
- Path → Channel mapping
- Secret validation for webhooks
- OAuth callback handling

---

## 4. Self-Correction & Reliability

### 4.1 Self-Repair ([`src/agent/self_repair.rs`](src/agent/self_repair.rs))

**[`SelfRepair`](src/agent/self_repair.rs:51) trait** - Interface for repair operations:
```rust
#[async_trait]
pub trait SelfRepair: Send + Sync {
    async fn detect_stuck_jobs(&self) -> Vec<StuckJob>;
    async fn repair_stuck_job(&self, job: &StuckJob) -> Result<RepairResult, RepairError>;
    async fn detect_broken_tools(&self) -> Vec<BrokenTool>;
    async fn repair_broken_tool(&self, tool: &BrokenTool) -> Result<RepairResult, RepairError>;
}
```

**[`RepairResult`](src/agent/self_repair.rs:38) enum**:
- `Success` - Repair completed
- `Retry` - Transient failure, retry later
- `Failed` - Permanent failure
- `ManualRequired` - Human intervention needed

**Repair flow**:
```
┌─────────────────────────────────────────────────────────────┐
│                    SELF-REPAIR CYCLE                        │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Every repair_check_interval:                               │
│                                                             │
│  STUCK JOBS:                                                │
│  1. Query ContextManager for stuck job IDs                  │
│  2. For each stuck job:                                     │
│     - Check repair_attempts < max_repair_attempts           │
│     - Call context.attempt_recovery()                       │
│     - Broadcast notification to all channels                │
│                                                             │
│  BROKEN TOOLS:                                              │
│  1. Query database for tools with 5+ failures               │
│  2. For each broken tool:                                   │
│     - Create BuildRequirement for repair                    │
│     - Call builder.build() to rebuild                       │
│     - Mark as repaired in database                          │
│     - Auto-register if successful                           │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 Kernel Monitor ([`src/agent/kernel_monitor.rs`](src/agent/kernel_monitor.rs))

**Performance monitoring and patch proposal system**:

**[`KernelMonitor`](src/agent/kernel_monitor.rs)**:
- Records tool execution samples (duration, success)
- Computes statistics (avg, p95, max)
- Detects bottlenecks (slow_threshold_ms)
- Proposes optimization patches

**[`PatchProposal`](src/agent/kernel_monitor.rs)**:
- `proposed_rust_code` - Generated optimization
- `expected_speedup` - Estimated improvement
- `status` - Pending, Approved, Deployed, Failed

### 4.3 Kernel Orchestrator ([`src/agent/kernel_orchestrator.rs`](src/agent/kernel_orchestrator.rs))

**[`KernelOrchestrator`](src/agent/kernel_orchestrator.rs)** - Coordinates monitoring and deployment:
- `tick()` - Periodic check for approved patches
- `deploy_approved_patches()` - Auto-deploy if configured
- `record_tool_execution()` - Feed data to monitor

### 4.4 Error Handling ([`src/error.rs`](src/error.rs))

**Comprehensive error taxonomy**:
- `ConfigError` - Configuration issues
- `DatabaseError` - Persistence failures
- `ChannelError` - Communication failures
- `LlmError` - LLM API issues
- `ToolError` - Tool execution failures
- `SafetyError` - Safety layer violations
- `JobError` - Job lifecycle issues
- `RepairError` - Self-repair failures
- `WorkspaceError` - File operations
- `OrchestratorError` - Container/sandbox issues
- `WorkerError` - Sandbox worker issues

---

## 5. Decision-Making & Autonomy

### 5.1 Routines ([`src/agent/routine.rs`](src/agent/routine.rs))

**[`Routine`](src/agent/routine.rs) struct** - Autonomous task definition:
```rust
pub struct Routine {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub user_id: String,
    pub enabled: bool,
    pub trigger: Trigger,
    pub action: RoutineAction,
    pub guardrails: RoutineGuardrails,
    pub notify: NotifyConfig,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub run_count: u32,
    pub consecutive_failures: u32,
    pub state: RoutineState,
}
```

**[`Trigger`](src/agent/routine.rs) enum**:
- `Cron { schedule }` - Standard cron expression
- `Event { pattern }` - Regex on incoming messages
- `Manual` - User-triggered only

**[`RoutineAction`](src/agent/routine.rs) enum**:
- `Lightweight { prompt, max_tokens }` - Quick LLM call
- `FullJob { job_description }` - Full job execution

**[`RoutineGuardrails`](src/agent/routine.rs)**:
- `cooldown` - Minimum time between runs
- `max_concurrent` - Parallel execution limit
- `dedup_window` - Prevent duplicate runs

### 5.2 Routine Engine ([`src/agent/routine_engine.rs`](src/agent/routine_engine.rs))

**[`RoutineEngine`](src/agent/routine_engine.rs)** - Execution coordinator:

```
┌─────────────────────────────────────────────────────────────┐
│                    ROUTINE ENGINE                           │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  TRIGGERS:                                                  │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐     │
│  │    Cron     │    │    Event    │    │   Manual    │     │
│  │   Ticker    │    │   Matcher   │    │   Trigger   │     │
│  └──────┬──────┘    └──────┬──────┘    └──────┬──────┘     │
│         │                  │                  │             │
│         └──────────────────┼──────────────────┘             │
│                            ▼                                │
│  EXECUTION:                                                │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  check_cooldown() → check_concurrent() → fire()     │   │
│  │                                                      │   │
│  │  Lightweight: LLM call → notification               │   │
│  │  FullJob: Schedule job → monitor → notification     │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│  EVENT CACHE: In-memory regex patterns for fast matching   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 5.3 Scheduler ([`src/agent/scheduler.rs`](src/agent/scheduler.rs))

**[`Scheduler`](src/agent/scheduler.rs)** - Task execution manager:

**Key features**:
- `schedule()` - Queue job for execution
- `spawn_subtask()` - Parallel task spawning
- `spawn_batch()` - Batch job processing
- Swarm offload for distributed execution

**[`OffloadDecision`](src/agent/scheduler.rs) enum**:
- `Local` - Execute on this node
- `Remote { assignee_node }` - Offload to swarm peer

### 5.4 Reflex Compiler ([`src/agent/reflex.rs`](src/agent/reflex.rs))

**Autonomous tool creation from recurring patterns**:

```
┌─────────────────────────────────────────────────────────────┐
│                    REFLEX COMPILER                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Every 5 minutes:                                           │
│                                                             │
│  1. Query database for recurring job patterns (3+ times)    │
│  2. For each pattern:                                       │
│     - Skip if already a known tool                          │
│     - Generate unique name: "reflex_<uuid8>"                │
│     - Create BuildRequirement:                              │
│       - description: "Handle: <pattern>"                    │
│       - software_type: WasmTool                             │
│       - capabilities: ["log"]                               │
│     - Call LlmSoftwareBuilder.build()                       │
│     - On success:                                           │
│       - Store pattern → tool mapping in database            │
│       - Tool auto-registered for future use                 │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

## 6. Supporting Systems

### 6.1 Heartbeat ([`src/agent/heartbeat.rs`](src/agent/heartbeat.rs))

**Proactive check-in system**:

**[`HeartbeatRunner`](src/agent/heartbeat.rs)**:
- Periodically reads workspace state
- Uses LLM to generate status update
- Notifies user if attention needed
- Tracks consecutive failures

**Attention detection**:
- Empty checklist items
- Stale files needing updates
- Outstanding tasks

### 6.2 Cost Guard ([`src/agent/cost_guard.rs`](src/agent/cost_guard.rs))

**Resource usage enforcement**:

**[`CostGuard`](src/agent/cost_guard.rs)**:
- `max_cost_per_day_cents` - Daily budget limit
- `max_actions_per_hour` - Rate limiting
- Tracks daily spend and hourly action count

**Enforcement**:
```rust
pub async fn check_allowed(&self) -> Result<(), CostLimitExceeded>
```

### 6.3 Context Monitor ([`src/agent/context_monitor.rs`](src/agent/context_monitor.rs))

**Token usage tracking**:

**[`ContextBreakdown`](src/agent/context_monitor.rs)**:
- `total_tokens` - Overall usage
- `system_tokens`, `user_tokens`, `assistant_tokens`, `tool_tokens`
- `message_count`

**Compaction triggers**:
- `DEFAULT_CONTEXT_LIMIT` = 128,000 tokens
- `COMPACTION_THRESHOLD` = 0.8 (80%)

### 6.4 Profile Synthesizer ([`src/agent/profile_synthesizer.rs`](src/agent/profile_synthesizer.rs))

**Dynamic learning from conversations**:

**[`ProfileSynthesizer`](src/agent/profile_synthesizer.rs)**:
- Enqueues conversation turns for analysis
- Background worker extracts observations
- Updates managed sections in identity files:
  - `AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, `MEMORY.md`

**Observation extraction**:
- Category classification (preference, behavior, goal)
- Confidence scoring
- Evidence excerpt tracking

---

## 7. Current Capabilities Assessment

### What TitanClaw Can Currently Do

| Capability | Status | Implementation |
|------------|--------|----------------|
| Multi-channel communication | ✅ Full | Discord, Slack, Telegram, WhatsApp, Web |
| Tool execution with approval | ✅ Full | WASM sandbox, safety layer |
| Session/thread management | ✅ Full | UUID mapping, persistence |
| Context compaction | ✅ Full | Summarize, truncate, archive |
| Self-repair | ✅ Full | Stuck jobs, broken tools |
| Autonomous routines | ✅ Full | Cron, event triggers |
| Reflex tool compilation | ✅ Full | Pattern detection → WASM |
| Profile synthesis | ✅ Full | Conversation learning |
| Distributed execution | ✅ Partial | Swarm offload, remote tasks |
| Health monitoring | ✅ Full | Heartbeat, kernel monitor |
| Cost guardrails | ✅ Full | Daily budget, hourly rate |

---

## 8. Architecture Strengths

### 8.1 Well-Designed Patterns

1. **Supervised Background Tasks** - Robust restart with backoff
2. **Hook System** - Extensible interception points
3. **Capability-Based Security** - WASM sandbox with explicit permissions
4. **Multi-LLM Support** - Primary + cheap LLM for different tasks
5. **Thread ID Preservation** - Avoids hydration mismatches
6. **Managed Sections** - Safe auto-updates to identity files
7. **Piped Execution** - Latency optimization for shell commands

### 8.2 Sophisticated Mechanisms

1. **Agentic Loop** - Proper tool iteration with forced tool use
2. **Tool Attenuation** - Skill-based tool filtering
3. **Event Cache** - Fast in-memory regex for routine triggers
4. **Patch Proposal System** - Performance optimization pipeline
5. **Auth Mode Interception** - Clean credential collection flow

---

## 9. Architecture Gaps for Jarvis-Level Autonomy

### 9.1 Missing Capabilities

| Gap | Current State | Needed for Jarvis |
|-----|---------------|-------------------|
| **Goal Management** | No persistent goal tracking | Hierarchical goal decomposition |
| **Planning Engine** | Single-turn decisions | Multi-step planning with replanning |
| **World Model** | No environment model | Causal model of systems |
| **Self-Reflection** | Basic repair only | Meta-cognitive evaluation |
| **Learning System** | Profile synthesis only | Continuous skill acquisition |
| **Proactive Action** | Heartbeat only | Autonomous task initiation |
| **Multi-Agent Coordination** | Swarm offload only | Collaborative problem solving |
| **Memory Architecture** | Simple workspace files | Episodic + semantic + working memory |
| **Reasoning Transparency** | Basic logging | Explainable decision chains |
| **Uncertainty Handling** | Binary success/fail | Confidence-weighted decisions |

### 9.2 Technical Debt

1. **No Persistent Goal State** - Goals are implicit in conversations
2. **Limited Memory Retrieval** - No semantic search over history
3. **No Planning Module** - Reactive, not proactive
4. **Single-Agent Focus** - No multi-agent orchestration
5. **No Self-Model** - Cannot reason about own capabilities

---

## 10. Key Files Reference

### Core Agent
| File | Lines | Purpose |
|------|-------|---------|
| [`src/agent/agent_loop.rs`](src/agent/agent_loop.rs) | 1155 | Main agent loop, startup, message handling |
| [`src/agent/dispatcher.rs`](src/agent/dispatcher.rs) | 1287 | Agentic loop, tool execution |
| [`src/agent/session.rs`](src/agent/session.rs) | ~900 | Session, thread, turn management |
| [`src/agent/session_manager.rs`](src/agent/session_manager.rs) | ~700 | Session resolution, thread mapping |
| [`src/agent/router.rs`](src/agent/router.rs) | ~300 | Message intent routing |

### Autonomy
| File | Lines | Purpose |
|------|-------|---------|
| [`src/agent/routine.rs`](src/agent/routine.rs) | ~500 | Routine definition, triggers |
| [`src/agent/routine_engine.rs`](src/agent/routine_engine.rs) | ~600 | Routine execution |
| [`src/agent/scheduler.rs`](src/agent/scheduler.rs) | ~900 | Task scheduling, swarm offload |
| [`src/agent/reflex.rs`](src/agent/reflex.rs) | 127 | Pattern → tool compilation |

### Reliability
| File | Lines | Purpose |
|------|-------|---------|
| [`src/agent/self_repair.rs`](src/agent/self_repair.rs) | 388 | Stuck job/tool repair |
| [`src/agent/kernel_monitor.rs`](src/agent/kernel_monitor.rs) | ~300 | Performance monitoring |
| [`src/agent/kernel_orchestrator.rs`](src/agent/kernel_orchestrator.rs) | ~150 | Patch deployment |
| [`src/error.rs`](src/error.rs) | ~300 | Error taxonomy |

### Memory
| File | Lines | Purpose |
|------|-------|---------|
| [`src/agent/compaction.rs`](src/agent/compaction.rs) | 324 | Context summarization |
| [`src/agent/context_monitor.rs`](src/agent/context_monitor.rs) | ~200 | Token tracking |
| [`src/agent/profile_synthesizer.rs`](src/agent/profile_synthesizer.rs) | ~500 | Conversation learning |

### WASM Tools
| File | Lines | Purpose |
|------|-------|---------|
| [`src/channels/wasm/loader.rs`](src/channels/wasm/loader.rs) | ~400 | Tool discovery/loading |
| [`src/channels/wasm/runtime.rs`](src/channels/wasm/runtime.rs) | ~300 | WASM execution |
| [`src/channels/wasm/host.rs`](src/channels/wasm/host.rs) | ~400 | Host state, rate limiting |
| [`src/channels/wasm/schema.rs`](src/channels/wasm/schema.rs) | ~500 | Capabilities schema |
| [`src/channels/wasm/router.rs`](src/channels/wasm/router.rs) | ~600 | HTTP routing, webhooks |

---

## 11. Recommendations for Jarvis-Level Autonomy

### Priority 1: Goal Management System
- Implement persistent goal storage with decomposition
- Add goal progress tracking and replanning
- Create goal-driven task prioritization

### Priority 2: Planning Engine
- Add multi-step planning with dependency resolution
- Implement plan monitoring and adjustment
- Create plan templates for common tasks

### Priority 3: Enhanced Memory
- Implement semantic search over conversation history
- Add episodic memory for experiences
- Create working memory for active tasks

### Priority 4: Self-Model
- Create capability registry with confidence scores
- Implement self-assessment mechanisms
- Add skill gap detection

### Priority 5: Proactive Architecture
- Extend heartbeat to autonomous task initiation
- Add opportunity detection
- Implement background goal pursuit

---

*Analysis completed: 2026-02-24*
*Architecture version: Current main branch*
