# TitanClaw UX Audit

**Date:** 2026-03-15
**Version:** 1.1.2

## Executive Summary

TitanClaw has a solid foundation with advanced features (autonomy, memory plane, tooling v2) but lacks polish and some key OpenClaw features. Priority: enable gated features, add missing conveniences, improve documentation.

## CLI Commands

### Available Commands

| Command | Status | Notes |
|---------|--------|-------|
| `run` | ✅ Works | Main interactive mode |
| `onboard` | ✅ Works | OpenClaw-style profile setup |
| `doctor` | ✅ Works | Dependency checks |
| `status` | ✅ Works | Health diagnostics |
| `config` | ✅ Works | Settings management |
| `tool` | ✅ Works | WASM tool management |
| `mcp` | ✅ Works | MCP server management |
| `memory` | ✅ Works | Workspace memory queries |
| `memory-plane` | ✅ Works | Memory Plane v2 ops |
| `goal` | ✅ Works | Autonomy goals CRUD |
| `plan` | ✅ Works | Autonomy plans CRUD |
| `plan-step` | ✅ Works | Plan steps CRUD |
| `pairing` | ✅ Works | DM pairing approval |
| `service` | ✅ Works | systemd/launchd integration |
| `worker` | ✅ Internal | Docker worker (internal) |
| `claude-bridge` | ✅ Internal | Claude Code bridge |
| `open-code-bridge` | ✅ Internal | OpenCode bridge |

### Missing vs OpenClaw

| OpenClaw Feature | TitanClaw Status | Priority |
|------------------|-----------------|----------|
| `gateway` command | ❌ Missing | Medium - web auto-starts |
| `browser` command | ❌ Missing | High - useful for agents |
| `canvas` command | ❌ Missing | Medium - UI rendering |
| `channels` command | ❌ Missing | High - Telegram/Discord config |
| `cron` command | 🟡 Partial | Routines exist, CLI missing |
| `agents` command | ❌ Missing | Low - subagent support |
| `approvals` command | ❌ Missing | Medium - approval management |
| `backup` command | ❌ Missing | Medium - state backup |

## Web Gateway

### Available Endpoints

```
Memory API:
  GET  /api/memory/tree
  GET  /api/memory/list
  GET  /api/memory/read
  POST /api/memory/write
  POST /api/memory/search

Jobs API:
  GET  /api/jobs
  GET  /api/jobs/summary
  GET  /api/jobs/{id}
  POST /api/jobs/{id}/cancel
  POST /api/jobs/{id}/restart
  POST /api/jobs/{id}/prompt
  GET  /api/jobs/{id}/events
  GET  /api/jobs/{id}/files/list
  GET  /api/jobs/{id}/files/read
  GET  /api/jobs/{id}/files/download

Goals API:
  GET  /api/goals
  GET  /api/goals/{id}
  POST /api/goals/{id}/status
  POST /api/goals/{id}/priority
  POST /api/goals/{id}/cancel
  POST /api/goals/{id}/complete
  POST /api/goals/{id}/abandon
  GET  /api/goals/{id}/policy-decisions
  GET  /api/goals/{id}/plans

Plans API:
  GET  /api/plans
  GET  /api/plans/{id}
  POST /api/plans/{id}/status
  POST /api/plans/{id}/cancel
  POST /api/plans/{id}/replan
  GET  /api/plans/{id}/verifications
  GET  /api/plans/{id}/executions

WebSocket:
  WS   /ws?token=...
```

### Web UI Comparison

| Feature | OpenClaw | TitanClaw |
|---------|----------|-----------|
| Chat interface | ✅ Full | ✅ Has handlers |
| Job management | ✅ Full | ✅ Full |
| Settings UI | ✅ | ✅ Has handlers |
| Skills browser | ✅ | ✅ Has handlers |
| Routines UI | ✅ | ✅ Has handlers |
| Extensions UI | ✅ | 🟡 Partial |
| Dashboard home | ✅ Rich | ❓ Unknown |
| Mobile responsive | ✅ | ❓ Unknown |

## UX Friction Points

1. **Onboarding blocking** - First run requires profile setup, no skip option
2. **No cron CLI** - Routines exist but no CLI to manage them
3. **No channel config CLI** - Telegram/Discord setup requires manual .env
4. **Gateway auto-starts** - Good for users, but no explicit control
5. **Missing browser tool** - No web automation from CLI
6. **Error messages** - Some Rust panic traces leak through

## Prioritized Improvements

### Quick Wins (1-2 hours each)
1. Add `--skip-onboard` flag to run command (already has `--no-onboard`)
2. Add `titanclaw cron` command wrapper around routines
3. Add `titanclaw channels` command for Telegram/Discord setup
4. Improve error messages (hide panic traces)

### Medium Effort (1-2 days each)
1. Add `titanclaw browser` command for web automation
2. Add `titanclaw backup` command for state export
3. Add `titanclaw approvals` command for pending approvals
4. Complete web UI polish (responsive, better errors)

### Major Work (1+ week each)
1. Add `titanclaw canvas` for UI rendering
2. Add `titanclaw agents` for subagent spawning
3. Full OpenClaw feature parity audit
4. Mobile app companion

## Recommendations

1. **Immediate**: Enable Memory Plane v2 and Tool Routing v2 flags
2. **Short-term**: Add cron, channels, backup commands
3. **Medium-term**: Add browser, approvals commands
4. **Long-term**: Canvas, agents, full parity

## Configuration Gaps

Current `.env.example` missing:
- `AUTONOMY_MEMORY_PLANE_V2=true`
- `AUTONOMY_TOOL_ROUTING_V2=true`
- `AUTONOMY_POLICY_ENGINE_V1=true`
- Channel tokens (Telegram, Discord) examples

## Documentation Gaps

Missing:
- Quick start guide
- CLI command reference
- Web API documentation
- Channel setup guide
- Autonomy features guide