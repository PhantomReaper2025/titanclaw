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
- Web gateway auth/render hardening:
  - SSE query-token auth accepts URL-encoded tokens
  - WebSocket Origin validation parses loopback hosts correctly (including IPv6)
  - chat markdown rendering uses DOM-based allowlist sanitization (no regex sanitizer / inline copy-button handlers)
