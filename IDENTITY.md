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
- Internal autonomy control-plane v1 persistence is now scaffolded across both PostgreSQL and libSQL backends (goals/plans/plan steps/execution attempts/policy decisions/incidents), with worker/dispatcher runtime paths best-effort writing records for planned worker runs and chat tool policy/execution events.

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
