# TitanClaw
Autonomous Agent Architecture Review and Jarvis-Grade Roadmap (12-Month, Supervised Autonomy)

## Summary

This plan reviews the current TitanClaw architecture (Rust-first, safety-oriented, multi-channel, WASM + Docker sandboxing, routines, self-repair, profile synthesis, swarm
offload) and defines a decision-complete roadmap to evolve it into a state-of-the-art autonomous agent platform.

The target is not "unsafe full autonomy." The chosen operating model is supervised autonomy for a personal-operator runtime over a 12-month phased roadmap. This is the
fastest path to surpassing OpenInterpreter-class systems on practical autonomy, reliability, and capability while preserving trust and controllability.

The roadmap focuses on six upgrades:

1. Goal-directed cognition (persistent goals, planning, replanning, execution policies)
2. Memory architecture overhaul (working + episodic + semantic + procedural memory with consolidation)
3. Tooling system v2 (typed contracts, capability trust, simulations, recovery semantics)
4. Self-correction loops v2 (critic loops, repair playbooks, canaries, shadow validation)
5. Reliability substrate (event-sourced execution records, idempotency, retries, observability)
6. Evaluation and governance (benchmarks, autonomy gates, safety thresholds, rollout controls)

## Architectural Review (Current State)

### What is already strong (keep and build on)

1. Agent runtime core is credible and extensible
   - Supervised background task pattern with restart/backoff is a strong foundation for long-lived autonomy.
   - References: src/agent/agent_loop.rs:64, src/agent/agent_loop.rs:360
2. Agentic loop has meaningful production features
   - Iterative tool loop, approval handling, hook interception, piped tool execution optimization, auth interception.
   - References: src/agent/dispatcher.rs:162, src/agent/dispatcher.rs:238, src/agent/dispatcher.rs:361, src/agent/dispatcher.rs:773
3. Session/thread reliability is ahead of many agent frameworks
   - External thread UUID preservation and explicit mapping reduces hydrate/resolve mismatches.
   - References: src/agent/session_manager.rs:94, AGENTS.md:39
4. Security-first execution model is a real differentiator
   - WASM sandboxing, Docker worker support, approvals, safety layer, capability-based controls.
   - This is strategically superior to "just shell + Python" designs.
5. Autonomy primitives already exist
   - Routines, reflex compilation, heartbeat, kernel monitor, self-repair.
   - These are strong seeds for a more advanced autonomous stack.
6. Profile synthesis and managed docs are a practical memory/product UX bridge
   - This gives user-aligned behavior grounding and long-term preference persistence.

### Gaps preventing Jarvis-level autonomy

1. No persistent goal/plan state machine
   - Current system is mostly reactive per turn/job/routine.
   - Missing explicit goal decomposition, progress tracking, replanning, and priority arbitration.
2. Memory is useful but not cognitively structured
   - Strong workspace and semantic infrastructure exist, but no formal split between working/episodic/procedural memory and no memory quality/consolidation loop.
3. Tool execution is powerful but not strategy-aware
   - Missing typed preconditions/postconditions, tool simulation/dry-run planning, fallback chains, and learned tool reliability scoring.
4. Self-correction exists but is narrow
   - Current self-repair and kernel monitor are valuable, but there is no generalized execution critic, hypothesis-driven recovery, or canary-based autonomous patch
     deployment pipeline.
5. Decision-making logic lacks explicit uncertainty and policy reasoning
   - No confidence-scored plans, risk-adjusted action selection, or dynamic escalation based on impact.
6. No unified autonomy governance layer
   - Approval exists, but not a centralized policy engine for "what is allowed autonomously under what evidence level."

### Accuracy note on the existing analysis doc

The existing plans/titanclaw-architecture-analysis.md is directionally useful but should not be treated as canonical without correction. It contains factual drift (for
example, it analyzes WASM channels as "tool integration" and misses current routine trigger details such as webhook triggers). Use code and AGENTS.md as source-of-truth.

## Target Architecture (Jarvis-Grade, Supervised Autonomy)

## Core design principle

Shift TitanClaw from a "turn-driven tool-using assistant" to a goal-driven autonomous operating system for agent work, with explicit state, policies, evidence, and
recovery.

## Target subsystem model

1. Cognitive Control Plane
   - Goal Manager
   - Planner/Replanner
   - Executor
   - Verifier/Critic
   - Policy Engine
   - Attention Manager (what to act on next)
2. Execution Plane
   - Tool Runtime (WASM, MCP, shell, HTTP, Docker)
   - Scheduler (local + swarm)
   - Routine Engine
   - Job Orchestrator
   - Sandbox/Worker fleet
3. Memory Plane
   - Working Memory (active task context)
   - Episodic Memory (time-ordered experiences and outcomes)
   - Semantic Memory (facts and embeddings)
   - Procedural Memory (skills, playbooks, successful plans)
   - Identity/Profile Memory (existing docs + managed sections)
4. Reliability & Self-Improvement Plane
   - Telemetry + Event Log
   - Incident Detector
   - Repair Engine
   - Shadow Execution
   - Patch Proposal/Canary/Deploy Pipeline
   - Evaluations and regression suite
5. Governance Plane
   - Risk classification
   - Capability policies
   - Approval policies
   - Audit trail
   - Safe mode / degraded mode controls

## Important Public API / Interface / Type Changes

These changes are required to make the roadmap implementable and coherent.

## New core domain types (Rust)

1. Goal
   - id, owner, title, intent, priority, status, deadline, constraints, risk_class, created_at, updated_at
2. GoalGraph
   - Parent/child goals, dependencies, blockers, completion criteria
3. Plan
   - id, goal_id, revision, steps, assumptions, confidence, estimated_cost, estimated_time, status
4. PlanStep
   - id, kind, inputs, tool_candidates, preconditions, postconditions, rollback, policy_requirements
5. ExecutionAttempt
   - id, plan_step_id, tool, args, result, latency, cost, status, failure_class, retry_count
6. Observation
   - Facts gathered during execution with provenance and confidence
7. PolicyDecision
   - decision, reason_codes, required_approval, risk_score, allowed_actions
8. CapabilityDescriptorV2
   - Unified tool/channel capabilities and constraints (separate adapter layer for channels vs tools)
9. ToolReliabilityProfile
   - Success rate, p95 latency, common failure modes, safe-fallback options
10. MemoryRecord

   - memory_type (working, episodic, semantic, procedural, profile), source, confidence, ttl, sensitivity

## Database / storage additions

1. goals
2. goal_edges
3. plans
4. plan_steps
5. execution_attempts
6. observations
7. policy_decisions
8. incidents
9. repair_actions
10. memory_events (write/consolidate/forget audit)
11. procedural_playbooks
12. tool_reliability_profiles

## API / command additions

1. CLI
   - titanclaw goal ...
   - titanclaw plan ...
   - titanclaw autonomy ... (policy profile, mode, limits)
   - titanclaw eval ...
   - titanclaw incident ...
2. Web/Gateway APIs
   - GET/POST /api/goals
   - GET/POST /api/plans
   - GET /api/executions/{id}
   - GET /api/incidents
   - POST /api/policies/simulate
   - POST /api/autonomy/mode
3. Tool interface extensions
   - Structured preconditions/postconditions
   - Dry-run capability flag
   - Idempotency hints
   - Side-effect classification
   - Compensation/rollback hints (optional)

## Memory Management Roadmap (State-of-the-Art)

## Target outcome

Memory becomes a first-class operating system service, not just file persistence plus retrieval. The agent should recall relevant facts, summarize experiences, reuse
procedures, and forget or down-rank stale/noisy data safely.

## Architecture changes

1. Memory tiering (mandatory)
   - Working memory: per-goal active scratchpad, bounded and explicit
   - Episodic memory: immutable event/experience records with outcomes
   - Semantic memory: facts/chunks + embeddings + provenance
   - Procedural memory: reusable strategies, prompts, tool sequences, playbooks
   - Profile memory: existing managed identity docs remain authoritative for user preference/persona constraints
2. Write policy engine
   - Every memory write goes through classification:
   - Is it fact, experience, preference, plan, or ephemeral?
   - Confidence score
   - Sensitivity label
   - TTL / retention policy
   - Provenance requirement
   - This prevents clutter and hallucinated memory writes.
3. Memory consolidation loop
   - Background process transforms episodic records into:
   - Durable facts (semantic memory)
   - Procedural playbooks (if repeated successful pattern)
   - Archived summaries (for long-term cost control)
   - Human-review-required entries for low-confidence but high-impact inferences
4. Memory retrieval orchestration
   - Retrieval must be task-type aware:
   - Troubleshooting task retrieves incidents, similar failures, tools, and recent environment observations
   - Coding task retrieves codebase context + prior implementation decisions + tool reliability data
   - Scheduling/admin tasks retrieve routines, goals, deadlines, and policy constraints
   - Retrieval composition should be deterministic and inspectable.
5. Forgetting and decay
   - Confidence decay for stale observations
   - Recency/utility-weighted eviction from working memory
   - No deletion of audit-critical records by default
   - "Soft forget" by demotion before deletion for personal runtime

## Implementation requirements

1. Reuse existing workspace hybrid search and embeddings as semantic memory substrate.
2. Add episodic/event store schema before building advanced planning.
3. Keep profile synthesis writing managed sections only.
4. Expose memory provenance in UI and logs to improve trust.

## Tool Integration Roadmap (Power + Safety)

## Target outcome

TitanClaw should become a high-trust orchestration engine that can choose, simulate, execute, and recover from tool calls across WASM, MCP, shell, and remote workers with
minimal operator intervention.

## Architecture changes

1. Tool Contract V2
   - Extend tool metadata with:
   - Side-effect level (read_only, workspace_write, system_mutation, external_mutation, financial, credential)
   - Idempotency (safe_retry, unsafe_retry, unknown)
   - Dry-run support (none, native, simulated)
   - Preconditions/postconditions
   - Timeout and retry guidance
   - Compensation strategy (optional)
2. Capability policy unification
   - Maintain distinct channel and tool runtimes internally, but introduce a unified policy/evaluation layer to avoid analysis/docs confusion and duplicated guardrail
     logic.
   - Canonicalize capability descriptors while preserving adapters for:
   - src/tools/wasm/*
   - src/channels/wasm/*
3. Tool selection with reliability priors
   - Rank tools by:
   - task fit
   - trust level
   - historical success rate
   - latency/cost
   - recent incident rate
   - approval likelihood
   - This materially improves autonomous completion rates.
4. Simulation-first planning
   - Planner should attempt dry-run/simulate before high-impact tool execution.
   - If no dry-run exists, use policy to enforce additional verification or approval.
5. Multi-tool transaction semantics (best effort)
   - For side-effectful workflows, track a saga-like execution chain with compensation steps where available.
   - Example: create file, commit, push, create PR; partial failure should produce explicit recovery path.
6. Tool health and adaptive fallback
   - Promote current self-repair into a tool fleet health subsystem:
   - circuit breaker per tool
   - cooldown
   - fallback alternative selection
   - degraded mode routing

## Self-Correction Loops Roadmap (Reliability + Independence)

## Target outcome

From "repair some failures" to "continuously detect, diagnose, recover, and improve" without unsafe self-modification.

## Architecture changes

1. Execution Critic Loop
   - Runs after every plan step and task completion
   - Evaluates:
   - goal progress
   - evidence sufficiency
   - contradictions
   - risk drift
   - cost/time budget drift
   - hallucination indicators
   - Emits corrective actions: retry, replan, ask user, gather evidence, rollback, escalate.
2. Incident taxonomy and root-cause pipeline
   - Standardize failure classes:
   - tool transient
   - tool permanent
   - policy denial
   - environment mismatch
   - plan error
   - memory retrieval miss
   - LLM reasoning error
   - auth/credential
   - Enables meaningful repair analytics and autonomy tuning.
3. Repair Playbooks (procedural memory)
   - Convert successful self-repair actions into reusable repair procedures with safety constraints.
   - This turns repair into a learning system without unsafe unrestricted code rewriting.
4. Shadow execution and canary validation
   - Expand existing shadow concepts:
   - simulate alternate plan/tool choices in parallel (low-cost where possible)
   - canary-run autonomously generated patches or procedural changes on limited scope
   - compare outcomes before broad rollout
5. Kernel monitor evolution
   - Keep patch proposals, but gate autonomous deploy by:
   - reproducible benchmark gain
   - test pass subset
   - canary result
   - policy approval threshold
   - human approval default remains required for code deployment in supervised mode
6. Autonomous recovery budgets
   - Limit retry storms and self-repair churn with budgets:
   - max retries per step
   - max replans per goal
   - max autonomous repair actions per hour
   - cooldown after repeated failure signatures

## Decision-Making Logic Roadmap (Planning, Reasoning, Control)

## Target outcome

Explicit, inspectable reasoning control without requiring chain-of-thought exposure. The agent should make better decisions through state machines, confidence, and policy,
not by "more prompt."

## Architecture changes

1. Goal Manager
   - Persistent goals with statuses:
   - proposed, active, blocked, waiting, completed, abandoned
   - Includes acceptance criteria and risk class.
   - All autonomous work attaches to a goal.
2. Planner / Replanner
   - Generate executable plans as structured PlanSteps.
   - Replan triggers:
   - failed step
   - new evidence
   - policy denial
   - resource budget breach
   - user interruption
   - environment drift
3. Attention Manager
   - Decides what to do next across:
   - active goals
   - routines
   - incidents
   - follow-ups
   - user requests
   - Uses weighted scoring (priority, urgency, confidence, expected value, risk, user recency).
4. Policy Engine
   - Central decision authority for autonomous actions.
   - Inputs:
   - action intent
   - tool metadata
   - goal risk class
   - confidence
   - environment mode
   - user autonomy posture
   - Output:
   - allow
   - allow-with-logging
   - require-approval
   - deny
   - require-more-evidence
5. Verifier / Evidence Gate
   - Prevents "completion theater."
   - Completion requires evidence matching acceptance criteria.
   - Example for coding task:
   - changed files match request
   - tests/lint run or explicitly skipped
   - outputs verify behavior
   - no unresolved policy violations
6. Uncertainty-aware decisions
   - Every plan and key action carries confidence score + uncertainty source.
   - Low confidence on high-impact actions automatically raises approval/evidence requirements.

## Strategic Roadmap (12 Months, Decision-Complete)

## Phase 0 (Weeks 1-4): Correctness of Foundations and Architecture Baseline

### Goals

1. Align documentation and architectural source-of-truth.
2. Create the event/telemetry basis for later autonomy.
3. Prevent planning work from building on inaccurate abstractions.

### Deliverables

1. Correct architecture analysis doc or replace with generated architecture inventory.
2. Add architecture conformance checks to CI (at minimum linting docs against file paths for major referenced modules).
3. Introduce ExecutionAttempt and PolicyDecision logging records around current dispatcher flow.
4. Define incident taxonomy enums and wire initial classification.

### Acceptance criteria

1. Every referenced subsystem in the architecture doc maps to real files/types.
2. Dispatcher emits structured execution and policy decision records for tool calls.
3. Incident records exist for failed tool calls and approval denials.

## Phase 1 (Months 2-3): Autonomy Control Plane v1 (Goals + Plans + Policies)

### Goals

1. Move from turn-centric to goal-centric execution.
2. Add explicit planning with policy-gated execution.

### Deliverables

1. Goal, Plan, PlanStep schemas and Rust types.
2. Goal/plan CLI and web APIs (create, list, inspect, cancel, reprioritize).
3. Planner v1 for common task classes:
   - coding change
   - research/summarization
   - system admin
   - routine automation
4. Policy engine v1 integrated into dispatcher and scheduler.
5. Verifier v1 for task completion evidence checks.

### Acceptance criteria

1. New autonomous actions require a Goal.
2. Plan execution can pause/resume/replan after step failure.
3. Policy decisions are logged and inspectable.
4. At least 80% of benchmarked multi-step local tasks complete with explicit plan trace.

## Phase 2 (Months 4-6): Memory Plane v2 (Structured Memory + Consolidation)

### Goals

1. Improve long-horizon continuity and reasoning quality.
2. Reduce repetition and brittle context dependence.

### Deliverables

1. Memory tiering implementation:
   - working memory store
   - episodic memory store
   - procedural playbook store
   - semantic retrieval orchestration over existing workspace substrate
2. Memory write policy engine with provenance/confidence/TTL.
3. Consolidation background loop (episodic -> semantic/procedural summaries).
4. Retrieval composer integrated with planner/executor by task type.
5. Memory inspection UI/API (provenance and confidence included).

### Acceptance criteria

1. Repeated multi-session tasks show improved completion rate without repeated user context.
2. Memory writes are classified and auditable.
3. Procedural playbooks are generated from repeated successful executions with opt-in/autonomy gating.
4. Retrieval latency remains within defined budget for interactive mode.

## Phase 3 (Months 7-9): Tooling System v2 + Reliability Loops v2

### Goals

1. Increase autonomous success rate on real work.
2. Reduce intervention required for transient failures and tool ecosystem fragility.

### Deliverables

1. Tool Contract V2 and metadata migration path.
2. Tool reliability profiles and ranking-aware selection.
3. Dry-run/simulation integration and policy rules for non-simulatable high-impact actions.
4. Execution Critic loop and replan hooks.
5. Incident detector + repair playbook engine.
6. Expanded shadow execution / canary framework for patch and workflow changes.

### Acceptance criteria

1. Autonomous task success rate improves materially over Phase 1 baseline.
2. Mean time to recovery decreases for transient tool failures.
3. Unsafe repeat retries are capped by policy budgets.
4. Canary policy prevents broad rollout of failing autonomous optimizations.

## Phase 4 (Months 10-12): Advanced Autonomy, Swarm Coordination, and Competitive Benchmarking

### Goals

1. Deliver a demonstrably superior personal autonomous agent runtime to OpenInterpreter-class systems.
2. Improve multi-agent orchestration and proactive goal pursuit while staying within supervised autonomy constraints.

### Deliverables

1. Attention Manager and cross-goal prioritization.
2. Swarm execution integration with plan steps and result validation contracts.
3. Proactive opportunity detection (goal-aligned, policy-limited) extending heartbeat and routines.
4. Benchmark suite and dashboard:
   - coding tasks
   - system automation tasks
   - recovery tasks
   - long-horizon continuity tasks
5. Autonomy modes:
   - conservative
   - balanced (default)
   - aggressive supervised

### Acceptance criteria

1. TitanClaw outperforms baseline OpenInterpreter-style workflows on:
   - multi-step completion rate
   - recovery rate after tool failures
   - repeat task efficiency
   - user intervention frequency
2. All proactive autonomous actions remain policy-audited and reversible where possible.
3. System operates stably for long-lived sessions with bounded memory growth and controlled retry behavior.

## Test Cases and Scenarios (Required)

## Functional autonomy tests

1. Multi-step coding task
   - Goal creation
   - plan generation
   - code changes
   - tests
   - failure on first test
   - replan
   - successful completion with evidence
2. System automation task with approvals
   - dry-run first
   - policy requires approval for destructive step
   - approval granted
   - execution proceeds
   - rollback path documented on partial failure
3. Routine-triggered autonomous maintenance
   - cron/event/webhook trigger
   - plan generated under goal
   - low-risk fix applied
   - summary recorded in episodic and procedural memory

## Reliability / recovery tests

1. Tool transient failure storm (timeouts/network)
   - circuit breaker engages
   - alternative tool selected
   - no runaway retries
2. Broken tool regression
   - incident classified
   - repair playbook invoked
   - canary validation before re-enabling
3. Swarm duplicate/replay conditions
   - dedupe behavior maintained
   - idempotent step handling verified

## Memory tests

1. Long-session context compaction + recall
   - facts retained with provenance
   - low-confidence inferences not promoted as facts
2. Procedural learning
   - repeated successful sequence becomes playbook
   - later task uses playbook and improves latency
3. Memory contamination defense
   - adversarial tool output attempts to inject false long-term memory
   - policy blocks or downgrades write

## Safety / policy tests

1. Destructive command without approval in supervised mode must be blocked.
2. High-confidence low-risk read-only actions may proceed autonomously and be audited.
3. Credential/auth interception flows remain consistent and do not leak secrets.

## Performance / ops tests

1. Long-lived agent runtime soak test (24h+)
2. Background supervisor restart behavior under panics
3. Memory retrieval latency budgets under load
4. Plan execution throughput local vs swarm-offloaded

## Metrics and Benchmarking (How to prove "state-of-the-art")

## Core KPIs

1. Autonomous task completion rate (end-to-end)
2. User intervention rate per task
3. Recovery success rate after first failure
4. Mean time to recovery
5. Plan rework rate (replans/task)
6. Memory retrieval precision and usefulness score
7. Tool success rate weighted by task class
8. Unsafe action prevention rate (policy effectiveness)
9. Cost per successful task
10. 24h runtime stability (crashes, supervisor restarts, degraded mode frequency)

## Competitive benchmark framing

Use reproducible task suites that compare TitanClaw against OpenInterpreter-class workflows on the same environment, emphasizing:

1. multi-step autonomy
2. failure recovery
3. long-horizon continuity
4. safe execution under policy constraints

## Assumptions and Defaults (Locked for this plan)

1. Autonomy posture: Supervised autonomy (chosen)
2. Primary environment: Personal operator runtime / small-team local-first deployment (chosen)
3. Planning horizon: 12-month staged roadmap (chosen)
4. Core language/runtime: Rust remains the control-plane foundation
5. Security posture: Approval remains mandatory for high-impact/destructive actions by default
6. Self-modification posture: Autonomous patch proposal allowed; autonomous deploy remains canary-gated and human-approved by default
7. Compatibility goal: Preserve current channels, routines, self-repair, and profile synthesis while refactoring behind stable interfaces
8. Documentation policy: Any behavior-changing roadmap execution must update implementation_plan.md and canonical docs per AGENTS.md:3

## Immediate Next Planning Artifact (to create before implementation)

Create a concrete "Autonomy Control Plane v1" spec that defines:

1. goal/plan schema and DB migrations
2. policy engine decision matrix
3. planner I/O contract
4. verifier evidence model
5. dispatcher integration points
6. acceptance benchmark set for Phase 1

This is the minimum decision-complete spec needed to begin implementation safely and avoid architecture drift.
