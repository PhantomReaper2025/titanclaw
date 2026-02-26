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
  - `agent_jobs` now persists optional autonomy linkage IDs (`autonomy_goal_id`, `autonomy_plan_id`, `autonomy_plan_step_id`) so worker/dispatcher correlation survives DB save/load and restarts (PostgreSQL + libSQL, `V17` + libSQL schema mirror update)
  - web gateway now exposes user-scoped autonomy goal/plan APIs for create + inspection + status updates (including goal reprioritization and convenience lifecycle aliases like cancel/complete/abandon/supersede), plus plan-step APIs for list/create/detail/status and atomic replace (`/api/plans/{id}/steps`, `/api/plans/{id}/steps/replace`, `/api/plan-steps/{id}`, `/api/plan-steps/{id}/status`)
  - web gateway now also exposes `POST /api/plans/{id}/replan` to create a next plan revision (optionally superseding the source plan, optionally copying source steps, or providing inline replacement steps in the same request)
  - web gateway also exposes user-scoped autonomy telemetry inspection endpoints for persisted execution attempts, plan verifications, and policy decisions (`GET /api/plans/{id}/executions`, `GET /api/plans/{id}/verifications`, `GET /api/goals/{id}/policy-decisions`), and plan-verification lists support filtering/sorting/pagination (`status`/`sort`/`offset`/`limit`)
  - CLI now exposes `titanclaw goal`, `titanclaw plan`, and `titanclaw plan-step` subcommands for user-scoped create/list/show/set-status/set-priority workflows, list filtering/pagination (`goal/plan list --status --sort --offset --limit`), `plan verifications` inspection (with `--status --sort --offset --limit`), plus convenience lifecycle aliases (`goal cancel|complete|abandon`, `plan cancel|complete|supersede`), `plan replan` (with optional `--copy-steps` or inline step payload via `--steps-file/--steps-json`), and atomic step replacement (`plan-step replace`)
- Web gateway auth/render hardening:
  - SSE query-token auth accepts URL-encoded tokens
  - WebSocket Origin validation parses loopback hosts correctly (including IPv6)
  - chat markdown rendering uses DOM-based allowlist sanitization (no regex sanitizer / inline copy-button handlers)
