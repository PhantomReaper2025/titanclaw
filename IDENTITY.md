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
- Runtime defaults are configured during onboarding and persisted as:
  - `CODING_RUNTIME_DEFAULT`
  - `OPENCODE_MODEL_DEFAULT`

## UX and Operations Identity

- Web chat supports lifecycle management:
  - delete single chat thread
  - clear all chats (chat records only)
- Sandbox job output is exportable directly from web UI via downloadable archive.

## Security Identity

- Sandbox-first by default
- Explicitly scoped user-owned data access in chat/job endpoints
- Secrets and provider auth are persisted through secure store or bootstrap env fallback
