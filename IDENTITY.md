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
