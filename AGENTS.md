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

## Current Product Baseline

- Product name: `TitanClaw`
- Sandbox coding modes: `worker`, `claude_code`, `opencode`
- Onboarding captures defaults for:
  - `CODING_RUNTIME_DEFAULT`
  - `OPENCODE_MODEL_DEFAULT`
- Web chat lifecycle controls:
  - delete one thread
  - clear all chats (chat scope only)
- Job output export:
  - archive download endpoint (`/api/jobs/{id}/files/download`)
