# Agent Rules

## Implementation Plan Update Policy

- If you change implementation status for any planned capability, update `implementation_plan.md` in the same branch.
- Do not open a PR that changes feature behavior without checking `implementation_plan.md` for needed status and notes updates.

## Canonical Docs Sync Policy (5 Files)

When behavior changes, keep these docs aligned in the same branch:

1. `AGENTS.md`
2. `IDENTITY.md`
3. `README.md`
4. `implementation_plan.md`
5. `CHANGELOG.md`

## Structured Job Completion Policy

- Sandbox jobs must use structured completion signaling (for example the worker `/complete` report / explicit status path) as the primary terminal-state mechanism.
- Free-text completion phrase matching is fallback-only for compatibility and should not be the primary source of truth for job completion.

## Current Product Baseline

- Product name: `TitanClaw`
- Sandbox coding modes: `worker`, `claude_code`, `opencode`
- WASM tool metadata resolution precedence:
  - loader sidecar override (`description`/`schema`, including `tool.*`)
  - WIT export introspection (`description()` / `schema()`)
  - explicit runtime fallback (warning-logged)
- Onboarding captures defaults for:
  - `CODING_RUNTIME_DEFAULT`
  - `OPENCODE_MODEL_DEFAULT`
- Conversational profile onboarding (OpenClaw-style):
  - soft-block first-chat flow after technical onboarding
  - runs in all channels via shared agent interception (web/CLI/Telegram)
  - supports `/onboard profile` commands (`status|defer|skip|reset`)
  - writes managed sections in `AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, `MEMORY.md`
- Session/thread reliability hardening:
  - gateway/web UUID external thread IDs are preserved during resolution to avoid hydrate/resolve mismatches (non-gateway channels remain channel-scoped)
  - stale/expired approval waits are auto-canceled (timeout) instead of blocking forever
  - conversation persistence now retries on DB failures and surfaces a user-visible warning if chat history still fails to save
  - approval rejection/resume paths keep thread/turn state consistent and persist resumed turn outcomes
- Profile synthesis (workspace core docs):
  - async post-turn with debounce
  - managed-section-only auto-updates (manual text outside markers is preserved)
  - targets `AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, `MEMORY.md`
- Web chat lifecycle controls:
  - delete one thread
  - clear all chats (chat scope only)
- Job output export:
  - archive download endpoint (`/api/jobs/{id}/files/download`) streams tar output (timeout-guarded) instead of buffering whole archives in memory
- Swarm runtime reliability:
  - scheduler remote offload now targets a selected assignee peer (task-level `assignee_node`) and receiving nodes ignore non-assigned task broadcasts
  - nodes dedupe repeated swarm task IDs with a bounded TTL cache to avoid duplicate execution on rebroadcast/replay
  - remote wait timeout is configurable via `SWARM_REMOTE_WAIT_TIMEOUT_MS` (clamped) with a safer default than the original `300ms`
- Background runtime supervision:
  - critical agent loops (self-repair, kernel monitor, shadow prune, reflex compiler) run under restartable supervisors with backoff and shutdown signaling
- Sandbox job completion signaling:
  - workers should terminate jobs through structured completion reports (`/worker/{job_id}/complete`) as the primary path
  - free-text completion phrase matching is fallback-only in worker loops (compatibility bridge)
- Sandbox job project directory ergonomics:
  - `create_job` now treats empty `project_dir` as omitted (auto-create behavior)
  - relative `project_dir` values (for example `portfolio`) are resolved and created under `~/.ironclaw/projects`
  - absolute `project_dir` values remain restricted to paths under `~/.ironclaw/projects`
  - chat-driven `create_job` calls now default to async start when `wait` is omitted
  - chat `job_started` status now includes optional `project_dir` context so users can locate artifacts faster
  - sandbox `create_job` now registers in-memory job context immediately so follow-up tools like `job_status`, `job_events`, and `job_prompt` can resolve the new job in the same runtime
  - waited sandbox jobs now require structured completion or a persisted terminal record; sandbox DB writes are awaited, stopped/missing handles get a short persisted-terminal grace window, and unconfirmed completion still fails instead of being treated as silent success
  - approval prompts now include policy notes for high-impact contract checks where explicit per-call approval is still required
- Approval/thread interaction hardening:
  - dispatcher tool-call hooks now run before approval checks, and approval-resume rechecks approval after hook mutation even when autonomy telemetry flags are off so rewritten parameters cannot bypass approval
  - pending approvals now terminalize and persist the affected turn on timeout or interrupt
  - expired approvals are now terminalized before `/undo`, `/redo`, `/resume`, or malformed web/gateway approval retries can leave them stuck
  - undo/redo/resume are blocked while a thread is `AwaitingApproval`, and gateway/web approval responses require request IDs
- Web approval/job/auth event scoping:
  - `approval_needed`, `job_started`, `auth_required`, and `auth_completed` SSE events now carry thread context for gateway chat filtering
  - gateway/web auth token submission now resolves the active thread when `thread_id` is omitted, so auth completion/retry events still return to the owning thread
  - web auth cards are keyed by extension plus thread, avoiding cross-thread removal when the same extension requests auth in multiple chats
  - approval cards submit the owning thread ID and are marked as submitted (not terminally resolved) until backend outcome events arrive
  - web socket approval submissions now emit explicit `Channel not started` / `Channel closed` errors instead of silently dropping failed sends
- Worker tool-call policy hardening:
  - worker now re-runs approval preflight after hook-based parameter mutation so rewritten params cannot bypass approval-required policy
- Shell execution reliability:
  - direct shell execution now drains stdout/stderr concurrently while the process runs to avoid deadlocks on large output
- Web gateway readiness contract:
  - gateway now exposes `GET /api/ready` with dependency checks (`message_pipeline`, `database_store`, `session_manager`) and returns `ready` vs `degraded`
- JIT tool production safety:
  - `jit_wasm_run` now always requires explicit approval
  - JIT tool registration is now explicit opt-in via `JIT_WASM_ENABLED=true` and still requires `ALLOW_LOCAL_TOOLS=true`
- Autonomy Control Plane v1 groundwork (internal persistence):
  - versioned autonomy domain types and dual-backend (PostgreSQL/libSQL) autonomy tables exist for goals/plans/plan steps/execution attempts/policy decisions/incidents/plan verifications
  - worker planned executions and dispatcher approval/tool-attempt paths now best-effort persist internal autonomy records (tracing emitters and approval UX remain unchanged), including worker post-plan verification outcomes for persisted plans with richer per-step checks/evidence and coverage for early completion-path exits
  - runtime control-plane v1 Phase 1 core is now complete for supervised worker/chat runtime paths: worker planning runs through a `PlannerV1` wrapper (validation + normalized plan trace), dispatcher/worker/approval-resume preflight decisions use shared policy-evaluation helpers (including hook re-check on approval resume and explicit approval/rejection policy decision persistence), worker completion uses a pre-completion `VerifierV1` soft gate, and planned worker runs can perform bounded automatic replanning (new plan revision persisted when autonomy linkage is available)
  - runtime policy-helper hardening now also covers piped shell early-start approval checks, reflex fast-path approval checks, and scheduler local/offload approval eligibility checks (shared decision semantics across these preflight paths)
  - verifier evidence classification now tags planned worker steps with richer evidence/check metadata (test/lint-check/diff/command categories), and `VerifierV1` uses this evidence to relax high-risk evidence gating when explicit validation/change evidence exists
  - runtime control-plane v1 rollout can be staged with internal flags: `AUTONOMY_POLICY_ENGINE_V1`, `AUTONOMY_VERIFIER_V1`, and `AUTONOMY_REPLANNER_V1` (all default-on)
  - targeted runtime acceptance coverage now includes worker auto-replan success (step failure -> replanned revision -> completion), worker policy-preflight decision persistence, and approval-resume approve/reject policy decision persistence assertions in `thread_ops`
  - Phase 2 Memory Plane v2 groundwork is now scaffolded internally: typed memory-plane domain types plus dual-backend (PostgreSQL/libSQL) persistence schema/store foundations for memory records/events/procedural playbooks/consolidation runs (`V19`-`V23` + libSQL mirror); runtime write/consolidation/retrieval integrations are still pending behind planned Phase 2 flags
  - Phase 2 Memory Plane v2 config/decision scaffolding now includes default-off runtime rollout flags (`AUTONOMY_MEMORY_PLANE_V2`, `AUTONOMY_MEMORY_RETRIEVAL_V2`, `AUTONOMY_MEMORY_CONSOLIDATION_V2` + interval/batch/TTL/playbook knobs) and an internal deterministic `MemoryWritePolicyEngine` classifier module (not wired to runtime write call-sites yet)
  - Phase 2 Memory Plane v2 now has initial flag-gated runtime episodic writes via the shared policy classifier helper: worker plan step outcomes, worker verifier outcomes, worker replan events, and worker/chat approval-policy decisions can be mirrored into `autonomy_memory_records` when `AUTONOMY_MEMORY_PLANE_V2=true` (fail-open, persistence best-effort)
  - Phase 2 Memory Plane v2 runtime write coverage now also includes scheduler tool-subtask outcome summaries (including local-fallback/remote-path hints) plus routine-run summaries and routine procedural playbook-candidate seed records after repeated successful runs, all behind `AUTONOMY_MEMORY_PLANE_V2`
  - Phase 2 Memory Plane v2 now includes a supervised, flag-gated `MemoryConsolidator` loop (`AUTONOMY_MEMORY_CONSOLIDATION_V2`) that records consolidation runs/events, processes active episodic memory records in batches, performs deterministic routine-summary -> semantic-summary promotion, and now also promotes routine playbook-candidate episodic records into `autonomy_procedural_playbooks` (create/update with archived source records and consolidation events)
  - Phase 2 Memory Plane v2 now includes a flag-gated `MemoryRetrievalComposer` (`AUTONOMY_MEMORY_RETRIEVAL_V2`) that performs task-aware memory/playbook selection and injects a transient retrieval context block into worker initial planning and automatic replanning (`PlannerV1` wrapper path); retrieval failures are fail-open and flag-off behavior preserves current prompts
  - Web gateway now exposes user-scoped Phase 2 Memory Plane v2 inspection/ops endpoints for memory records/playbooks plus playbook status updates, consolidation run listing/manual trigger, and retrieval preview debugging (`/api/memory-plane/*`) with filter/sort/pagination validation and ownership enforcement
  - CLI now exposes `titanclaw memory-plane` subcommands for Memory Plane v2 records/playbooks inspection, playbook status updates, consolidation run listing/manual trigger, and retrieval preview (`records list|show`, `playbooks list|show|set-status`, `consolidation runs|run`, `retrieval preview`) with validation semantics aligned to the gateway
  - Phase 2 Memory Plane v2 is now functionally complete behind default-off rollout flags (`AUTONOMY_MEMORY_PLANE_V2`, `AUTONOMY_MEMORY_RETRIEVAL_V2`, `AUTONOMY_MEMORY_CONSOLIDATION_V2`): typed persistence + write policy + runtime write mirrors + supervised consolidation + retrieval composer injection + gateway/CLI inspection/ops surfaces, with acceptance/stability tests covering worker/thread runtime memory writes and flag-off behavior
  - Phase 3 Tooling System v2 / reliability foundations (slice 1) are now in place internally: schema migrations `V24`-`V27` plus libSQL mirror/compatibility patching extend `autonomy_incidents` with dedupe/reliability fields (`fingerprint`, `surface`, `tool_name`, occurrence/first-seen/last-seen/failure-class), add `tool_contract_v2_overrides` and `tool_reliability_profiles`, add an `AutonomyReliabilityStore` DB subtrait with PostgreSQL/libSQL CRUD for incidents/contract overrides/reliability profiles, and introduce typed Tool Contract V2 / reliability domain structs in `src/tools/contract_v2.rs` (runtime resolver/ranking/incident-detector/critic integration remains pending later Phase 3 slices)
  - Phase 3 Tooling System v2 / reliability foundations (slice 2) now add Tool Contract V2 descriptor inference + overlay resolution helpers (`infer_tool_contract_v2_descriptor`, `resolve_contract_v2_descriptor_overlay`) and `ToolRegistry` resolver APIs (`infer_tool_contract_v2`, `resolve_tool_contract_v2`, `resolve_tool_contracts_v2`) that apply DB-backed overrides with precedence `user override -> global override -> inferred` (no execution-path behavior changes yet; ranking/critic/runtime integration remains pending)
  - Phase 3 Tooling System v2 / reliability foundations (slice 3) now add a conservative `ToolReliabilityService` scaffold (`src/agent/tool_reliability.rs`) with deterministic reliability-score + circuit-breaker profile computation from persisted autonomy execution attempts/policy decisions/incidents plus a legacy `tool_failures` bridge penalty, along with a new cross-backend `AutonomyExecutionStore::list_execution_attempts_for_user(...)` helper used for manual profile recompute paths (runtime ranking/fallback enforcement remains pending)
  - Phase 3 Tooling System v2 / reliability foundations (slice 4) now add shared runtime incident detection integration (`src/agent/incident_detector.rs`) with deterministic fingerprinting + open-incident dedupe, wired into worker/dispatcher failed execution-attempt persistence and worker/dispatcher/thread deny-policy persistence so autonomy incidents are recorded with linkage (`execution_attempt_id` / `policy_decision_id`) and occurrence increments (runtime ranking/fallback/critic enforcement remains pending)
  - Phase 3 Tooling System v2 / reliability foundations (slice 5) now auto-refresh tool reliability profiles from live runtime persistence paths: worker/dispatcher execution-attempt writes and worker/dispatcher/thread policy-decision writes trigger best-effort `ToolReliabilityService` recomputation for the touched tool (`recompute_tool_reliability_profile_best_effort`), keeping `tool_reliability_profiles` current without manual recompute commands
  - Phase 3 Tooling System v2 / reliability foundations (slice 6) now add a default-off worker routing flag (`AUTONOMY_TOOL_ROUTING_V2`) and reliability-aware planned-step fallback routing: candidate ordering uses profile `safe_fallback_options` + breaker state, open-breaker primaries can route to safer alternatives, and per-attempt worker execution persistence now records retry counts across fallback attempts
  - Phase 3 Tooling System v2 / reliability foundations (slice 7) now extend `AUTONOMY_TOOL_ROUTING_V2` beyond planned worker steps: dispatcher tool calls and thread approval/resume/reflex paths resolve fallback-or-deny routing from reliability profiles, policy decisions are persisted for routing modifications/denials, piped early shell execution is disabled while routing is enabled to avoid bypass, and both chat tool execution + non-planned worker tool execution hard-block open-breaker tools with explicit deny telemetry
  - Phase 3 Tooling System v2 / reliability foundations (slice 8) now wire Tool Contract V2 metadata into runtime policy evaluation: dispatcher/worker/scheduler/reflex preflight approval checks resolve contract descriptors (including user/global overrides when available), high-impact contract side-effect classes enforce explicit approval semantics, and worker autonomous execution now blocks contract-flagged approval-required tools with persisted policy reason codes
  - Phase 3 Tooling System v2 / reliability foundations (slice 9) now add dry-run/simulation policy semantics to Tool Contract V2 checks: high-impact simulatable actions can be preflight-allowed only when explicit dry-run intent is present (`dry_run`/`simulate`/`mode=dry_run`), while non-simulatable high-impact actions are explicitly tagged (`contract_v2_non_simulatable_high_impact`) and forced through approval policy paths
  - Phase 3 Tooling System v2 / reliability foundations (slice 10) now add an explicit `ExecutionCritic` loop in planned worker execution (`src/agent/execution_critic.rs`): each step result is post-evaluated for policy blocks/transient/tool-availability/latency-drift signals, and critic-triggered replan hooks can augment or override step-derived replan reason selection (for example escalating StepFailure to PolicyDenied) with reason-code payload propagation into replan memory events
  - Phase 3 Tooling System v2 / reliability foundations (slice 11) now add contract-aware fallback candidate composition and safety-weighted ranking in routing paths: worker planned-step routing and chat routing merge fallback options from reliability profiles plus Tool Contract V2 `fallback_candidates`, dedupe deterministically, and score alternatives by reliability plus contract risk/idempotency bias (favoring safer retryable tools) before fallback selection
  - Phase 3 Tooling System v2 / reliability foundations (slice 12) now add proactive degraded-tool rerouting: when a primary tool has sufficient poor reliability evidence (or half-open breaker state) and a materially better fallback exists, worker planned-step routing and chat routing can prefer the fallback before any primary attempt (`tool_routing_v2_prefer_reliable_fallback`), while preserving existing open-breaker hard-block behavior
  - Smart Rules v1 (default-on) now adds conversation-first recovery and adaptive routing knobs: planner empty-action outputs are converted into clarification guidance (instead of opaque failure), worker direct-selection no-action text is treated as clarification-needed guidance, chat can replace non-actionable "no task" style model replies with concrete follow-up prompts, and routing/retry thresholds are tunable via `AUTONOMY_SMART_RULES_*` env/config values (`ENABLED`, proactive sample/score/margin knobs, fallback-attempt budget, empty-plan recovery toggle)
  - `agent_jobs` now persists optional autonomy linkage IDs (`autonomy_goal_id`, `autonomy_plan_id`, `autonomy_plan_step_id`) so worker/dispatcher correlation survives DB save/load and restarts (PostgreSQL + libSQL, `V17` + libSQL schema mirror update)
  - web gateway now exposes user-scoped autonomy goal/plan APIs for create + inspection + status updates (including goal reprioritization and convenience lifecycle aliases like cancel/complete/abandon/supersede), plus plan-step APIs for list/create/detail/status and atomic replace (`/api/plans/{id}/steps`, `/api/plans/{id}/steps/replace`, `/api/plan-steps/{id}`, `/api/plan-steps/{id}/status`)
  - web gateway now also exposes `POST /api/plans/{id}/replan` to create a next plan revision (optionally superseding the source plan, optionally copying source steps, or providing inline replacement steps in the same request)
  - web gateway also exposes user-scoped autonomy telemetry inspection endpoints for persisted execution attempts, plan verifications, and policy decisions (`GET /api/plans/{id}/executions`, `GET /api/plans/{id}/verifications`, `GET /api/goals/{id}/policy-decisions`), and plan-verification lists support filtering/sorting/pagination (`status`/`sort`/`offset`/`limit`)
  - CLI now exposes `titanclaw goal`, `titanclaw plan`, and `titanclaw plan-step` subcommands for user-scoped create/list/show/set-status/set-priority workflows, list filtering/pagination (`goal/plan list --status --sort --offset --limit`), `plan verifications` inspection (with `--status --sort --offset --limit`), plus convenience lifecycle aliases (`goal cancel|complete|abandon`, `plan cancel|complete|supersede`), `plan replan` (with optional `--copy-steps` or inline step payload via `--steps-file/--steps-json`), and atomic step replacement (`plan-step replace`)
- Web gateway auth/render hardening:
  - SSE query-token auth accepts URL-encoded tokens
  - WebSocket Origin validation parses loopback hosts correctly (including IPv6)
  - chat markdown rendering uses DOM-based allowlist sanitization (no regex sanitizer / inline copy-button handlers)
