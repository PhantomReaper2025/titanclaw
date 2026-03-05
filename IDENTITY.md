# TitanClaw Identity

## Name and Positioning

- Product: `TitanClaw`
- Lineage: built on IronClaw architecture
- Goal: secure local-first AI orchestration with production-ready sandbox execution

## Runtime Identity

- Primary sandbox coding runtimes:
  - `worker` (built-in TitanClaw agent loop)
  - `claude_code` (Claude CLI bridge)
  - `opencode` (OpenCode-oriented runtime path)
- WASM tools expose LLM-facing metadata using:
  - sidecar schema/description overrides when provided
  - WIT export introspection (`description()` / `schema()`) when available
  - warning-logged fallback metadata as last resort
- Runtime defaults are configured during onboarding and persisted as:
  - `CODING_RUNTIME_DEFAULT`
  - `OPENCODE_MODEL_DEFAULT`
- Internal autonomy control-plane v1 persistence is now scaffolded across both PostgreSQL and libSQL backends (goals/plans/plan steps/execution attempts/policy decisions/incidents/plan verifications), with worker/dispatcher runtime paths best-effort writing records for planned worker runs and chat tool policy/execution events, plus worker post-plan verification outcomes for persisted plans (including richer per-step checks/evidence and early completion-path coverage).
- Internal autonomy runtime control-plane v1 Phase 1 core is now active (supervised worker/chat runtime paths): worker planning runs through a `PlannerV1` wrapper (plan validation + normalized trace), dispatcher/worker/approval-resume tool preflight uses shared policy-evaluation helpers (including hook re-checks before approved resume execution and explicit approve/reject policy-decision persistence), planned worker completion is gated by a pre-completion `VerifierV1` soft gate, and planned worker runs can perform bounded automatic replanning with persisted plan revisions when autonomy linkage is available.
- Runtime preflight policy semantics are now more consistent across edge paths (`piped` shell early-start, reflex fast-path tool execution, and scheduler local/offload eligibility) by reusing the same approval-evaluation helpers.
- Verifier evidence handling is now richer: worker plan-step evidence records include explicit categories (test/lint-check/diff/command), and `VerifierV1` uses those signals to distinguish high-risk completions that have real validation/change evidence from those that need more evidence.
- Runtime control-plane v1 rollout/stabilization supports internal feature flags (`AUTONOMY_POLICY_ENGINE_V1`, `AUTONOMY_VERIFIER_V1`, `AUTONOMY_REPLANNER_V1`) so policy/verifier/replanner behavior can be disabled independently while keeping persistence/CRUD surfaces intact.
- Runtime Phase 1 acceptance coverage includes worker auto-replan success and direct persistence assertions for worker policy-preflight plus approval-resume approve/reject policy decisions.
- Phase 2 Memory Plane v2 groundwork is now scaffolded internally: typed memory-plane domain records and dual-backend (PostgreSQL/libSQL) persistence schema/store support for memory records/events/procedural playbooks/consolidation runs (`V19`-`V23` + libSQL mirror) are in place, with runtime write/consolidation/retrieval integrations still pending behind Phase 2 flags.
- Phase 2 Memory Plane v2 rollout/config scaffolding is now present: default-off runtime flags (`AUTONOMY_MEMORY_PLANE_V2`, `AUTONOMY_MEMORY_RETRIEVAL_V2`, `AUTONOMY_MEMORY_CONSOLIDATION_V2` and tuning knobs) plus an internal deterministic `MemoryWritePolicyEngine` module for classification/provenance/TTL decisions (runtime call-site wiring still pending).
- Phase 2 Memory Plane v2 runtime write integration has started behind `AUTONOMY_MEMORY_PLANE_V2`: worker planned step outcomes/verifier outcomes/replan events and worker+chat approval policy decisions now have best-effort episodic `autonomy_memory_records` mirrors via a shared classifier-backed helper (fail-open on memory persistence errors).
- Phase 2 Memory Plane v2 runtime write coverage now extends to scheduler tool-subtask outcomes (including local-fallback/remote-path hints) and routine engine summaries; repeated successful routine runs can also seed procedural playbook-candidate records (still flag-gated, best-effort).
- Phase 2 Memory Plane v2 consolidation is now scaffolded and supervised in the agent runtime (`AUTONOMY_MEMORY_CONSOLIDATION_V2`): a `MemoryConsolidator` loop records consolidation runs/events, batches active episodic records, performs deterministic routine-summary -> semantic-summary promotion, and now also promotes routine playbook-candidate episodic records into procedural playbooks (`autonomy_procedural_playbooks`) with create/update + source-archive tracking.
- Phase 2 Memory Plane v2 retrieval integration is now scaffolded behind `AUTONOMY_MEMORY_RETRIEVAL_V2`: a task-aware `MemoryRetrievalComposer` selects relevant memory records/playbooks and injects a transient memory-context block into worker initial planning and automatic replanning prompts (fail-open on retrieval errors, no prompt changes when disabled).
- Phase 2 Memory Plane v2 now also has user-scoped gateway inspection/ops APIs for memory records/playbooks, playbook status updates, consolidation run listing/manual trigger, and retrieval preview debugging (`/api/memory-plane/*`), with ownership checks plus filter/sort/pagination validation.
- Phase 2 Memory Plane v2 now also has CLI inspection/ops parity via `titanclaw memory-plane ...` for records/playbooks/consolidation/retrieval preview workflows, using user-scoped ownership checks and validation rules aligned with the gateway endpoints.
- Phase 2 Memory Plane v2 is now functionally complete behind default-off flags: typed persistence, deterministic memory write policy, flag-gated runtime write mirrors, supervised consolidation (semantic + procedural deterministic promotions), retrieval composer injection into worker planning/replanning, and gateway/CLI inspection+ops surfaces with acceptance/stability coverage (including worker/approval memory-write assertions and flag-off behavior checks).
- Phase 3 Tooling System v2 / reliability foundations (slice 1) are now in place internally: migrations `V24`-`V27` and the libSQL consolidated-schema mirror/compatibility path extend `autonomy_incidents` with dedupe/reliability fields and add `tool_contract_v2_overrides` plus `tool_reliability_profiles`; the DB layer now includes an `AutonomyReliabilityStore` subtrait with PostgreSQL/libSQL CRUD for incidents, tool contract overrides, and tool reliability profiles, plus typed Tool Contract V2 / circuit-breaker reliability structs in `src/tools/contract_v2.rs` (resolver/ranking/incident-detector/critic runtime integrations remain pending Phase 3 follow-up slices).
- Phase 3 Tooling System v2 / reliability foundations (slice 2) now add pure Tool Contract V2 inference/overlay logic and `ToolRegistry` resolver APIs that can return inferred descriptors or apply DB-backed overrides with explicit precedence (`user` -> `global` -> inferred), establishing a stable internal descriptor-resolution seam before ranking/reliability/critic runtime integrations.
- Phase 3 Tooling System v2 / reliability foundations (slice 3) now include a conservative `ToolReliabilityService` scaffold that computes persisted reliability profiles (success/failure/timeout/blocked counts, latency percentiles, policy-friction and incident penalties, score, and circuit-breaker state) from existing autonomy execution/policy/incident records plus a legacy `tool_failures` bridge penalty; this is currently a manual recompute/internals seam and does not yet change runtime routing behavior.
- Phase 3 Tooling System v2 / reliability foundations (slice 4) now add shared runtime incident-detection integration (`src/agent/incident_detector.rs`) with deterministic fingerprint dedupe + occurrence increments, wired into worker/dispatcher failed execution-attempt persistence and worker/dispatcher/thread deny-policy persistence so incidents are linked to autonomy records (`execution_attempt_id` / `policy_decision_id`) while staying fail-open.
- Phase 3 Tooling System v2 / reliability foundations (slice 5) now auto-refresh reliability profiles on runtime persistence events: worker/dispatcher execution-attempt writes and worker/dispatcher/thread policy-decision writes trigger best-effort `ToolReliabilityService` recomputation for the touched tool, keeping `tool_reliability_profiles` continuously updated.
- Phase 3 Tooling System v2 / reliability foundations (slice 6) now add worker-side reliability-aware fallback routing behind default-off `AUTONOMY_TOOL_ROUTING_V2`: planned-step execution can rank primary + configured safe fallback tools using profile breaker state/reliability metadata, retry through alternatives, and persist per-attempt execution telemetry with retry counts.
- Phase 3 Tooling System v2 / reliability foundations (slice 7) now extend `AUTONOMY_TOOL_ROUTING_V2` into interactive runtime paths: dispatcher tool calls and thread reflex/approval-resume flows perform reliability-based fallback-or-deny routing with persisted policy decisions, piped early shell execution is disabled while routing is enabled to avoid bypass, and open-breaker tools are hard-blocked in both chat and non-planned worker execution paths with explicit deny telemetry.
- Phase 3 Tooling System v2 / reliability foundations (slice 8) now integrate Tool Contract V2 into runtime policy semantics: dispatcher/worker/scheduler/reflex preflight approval checks resolve contract descriptors (including override precedence when DB-backed overrides exist), contract-declared high-impact side-effect levels enforce explicit approval, and worker autonomous tool execution now blocks contract-driven approval-required tools with persisted policy reason codes.
- Phase 3 Tooling System v2 / reliability foundations (slice 9) now add dry-run/simulation policy semantics to contract evaluation: high-impact simulatable actions are treated as preflight-safe only when explicit dry-run intent is provided in parameters, while non-simulatable high-impact actions are explicitly reason-coded and continue through strict approval-required policy paths.
- Phase 3 Tooling System v2 / reliability foundations (slice 10) now add an explicit execution critic loop module (`ExecutionCritic`) for planned worker steps: post-step deterministic checks can trigger replan hooks for policy-denied/transient/tool-unavailable/latency-drift signals, and critic decisions now feed replan reason/code payloads in worker runtime memory events.
- Phase 3 Tooling System v2 / reliability foundations (slice 11) now add contract-aware fallback ranking in runtime routing: worker planned-step routing and chat fallback routing merge reliability profile fallback options with Tool Contract V2 `fallback_candidates`, then score/dedupe alternatives using reliability plus contract safety/idempotency bias to prefer lower-risk retry-safe fallbacks.
- Phase 3 Tooling System v2 / reliability foundations (slice 12) now add proactive degraded-primary rerouting: if a primary tool is sufficiently degraded (or half-open) and a stronger fallback is available, worker planned-step routing and chat routing can choose the fallback first before attempting the primary, with explicit routing reason codes and existing open-breaker deny semantics preserved.
- Smart Rules v1 is now default-on for runtime intent recovery: planner empty-action outputs are classified as clarification-needed (not silent dead-end errors), worker no-action model replies produce explicit clarification guidance, chat non-actionable replies can be rewritten into actionable follow-up prompts, and adaptive routing/retry behavior is tunable through `AUTONOMY_SMART_RULES_*` knobs (`ENABLED`, proactive thresholds, fallback-attempt budget, empty-plan recovery).
- Job records now persist optional autonomy linkage IDs (`autonomy_goal_id`, `autonomy_plan_id`, `autonomy_plan_step_id`) across PostgreSQL and libSQL so worker/dispatcher autonomy records can remain correlated after DB reloads/restarts.
- Web gateway exposes user-scoped autonomy goal/plan APIs for creation, inspection, status updates, and goal reprioritization (including convenience lifecycle endpoints such as cancel/complete/abandon/supersede), and now includes plan-step APIs (`GET/POST /api/plans/{id}/steps`, `POST /api/plans/{id}/steps/replace`, `GET /api/plan-steps/{id}`, `POST /api/plan-steps/{id}/status`) for structured step management and atomic step replacement during replans.
- Goal/plan list inspection endpoints now support lightweight filtering/pagination/sorting (`status`, `sort`, `offset`, `limit`) for user-scoped dashboard and CLI parity use cases.
- Web gateway now also supports `POST /api/plans/{id}/replan` to create the next plan revision with optional metadata overrides, optional superseding of the source plan, and either optional source-step copying or inline provided steps for the new revision.
- Web gateway now also exposes user-scoped telemetry inspection for persisted autonomy execution attempts, plan verifications, and policy decisions (`GET /api/plans/{id}/executions`, `GET /api/plans/{id}/verifications`, `GET /api/goals/{id}/policy-decisions`) to validate runtime instrumentation; plan-verification lists and goal-scoped plan lists (`GET /api/goals/{id}/plans`) now support filtering/sorting/pagination (`status`, `sort`, `offset`, `limit`).
- CLI exposes `titanclaw goal`, `titanclaw plan`, and `titanclaw plan-step` subcommands (including `goal set-priority`, `goal/plan list --status --sort --offset --limit`, `plan verifications --status --sort --offset --limit`, lifecycle aliases like `goal cancel|complete|abandon` and `plan cancel|complete|supersede`, plus `plan replan --copy-steps` or `plan replan --steps-file/--steps-json`, and `plan-step replace`) for direct inspection and manual management of autonomy records in local/single-user workflows.

## UX and Operations Identity

- Two-stage onboarding:
  - technical onboarding wizard (`titanclaw onboard`) for infrastructure/provider/channel setup
  - conversational profile onboarding in chat (OpenClaw-style) for identity/goals/tone/work style
- Web chat supports lifecycle management:
  - delete single chat thread
  - clear all chats (chat records only)
- Sandbox job output is exportable directly from web UI via downloadable archive.
- Archive downloads are streamed from the gateway with a timeout guard (no full in-memory archive buffering).
- Sandbox jobs should reach terminal state via structured worker completion reports (`/worker/{job_id}/complete`); text completion phrase matching is fallback-only inside worker loops.
- Swarm offload now targets a selected assignee peer and ignores non-assigned task broadcasts, reducing duplicate remote execution in multi-node meshes.
- Critical agent background loops (self-repair, kernel monitor, shadow pruning, reflex compiler) are supervised and restart with backoff on unexpected exit.
- Workspace core identity docs (`AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, `MEMORY.md`)
  support managed auto-updated sections populated by background profile synthesis
  after successful turns; manual content outside managed markers is preserved.
- Conversational profile onboarding also writes managed baseline sections into the same core docs
  after a review + confirm step.

## Security Identity

- Sandbox-first by default
- Explicitly scoped user-owned data access in chat/job endpoints
- Secrets and provider auth are persisted through secure store or bootstrap env fallback
- Approval waits are no longer unbounded in session memory; expired approval requests are canceled and the thread returns to idle/error-safe flow.
- Conversation-history persistence is now retried on transient DB failures and emits an in-channel warning if persistence still fails after retries (execution result is not silently treated as durable).
- Web gateway auth/rendering is hardened: URL-encoded SSE query tokens are accepted, WebSocket `Origin` is parsed and restricted to loopback hosts, and rendered chat markdown is DOM-sanitized via an allowlist.
