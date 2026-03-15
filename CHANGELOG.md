# Changelog

All notable changes to TitanClaw will be documented in this file.

## [1.2.0] - 2026-03-16

### Added
- **CLI Commands** - Full parity with OpenClaw:
  - `titanclaw cron list/add/rm/run/status` - Manage scheduled routines
  - `titanclaw channels list/status/available/install` - Manage chat channels
  - `titanclaw backup create/restore/list/export/import` - Backup/restore agent state
  - `titanclaw browser status/start/stop/open/tabs/snapshot/screenshot/act` - Browser automation
  - `titanclaw approvals list/show/approve/deny/auto-approve/rules` - Manage pending approvals
  - `titanclaw agents list/spawn/status/send/kill/wait/logs` - Subagent management
  - `titanclaw canvas status/present/hide/navigate/eval/snapshot/screenshot` - UI rendering

- **Documentation**:
  - `docs/QUICK_START.md` - 5-minute setup guide
  - `docs/FEATURE_STATUS.md` - Feature completion analysis
  - `docs/UX_AUDIT.md` - UX comparison with OpenClaw

- **Configuration**:
  - `.env` with Ollama + libSQL configuration
  - Enabled Memory Plane v2, Tool Routing v2, Autonomy Control Plane v1

### Changed
- Improved CLI command organization
- Added comprehensive help text for all commands

### Technical
- All advanced features now configurable via environment flags:
  - `AUTONOMY_POLICY_ENGINE_V1=true`
  - `AUTONOMY_VERIFIER_V1=true`
  - `AUTONOMY_REPLANNER_V1=true`
  - `AUTONOMY_MEMORY_PLANE_V2=true`
  - `AUTONOMY_MEMORY_RETRIEVAL_V2=true`
  - `AUTONOMY_MEMORY_CONSOLIDATION_V2=true`
  - `AUTONOMY_TOOL_ROUTING_V2=true`

## [1.1.2] - 2026-03-05

### Added
- Autonomy Control Plane v1 groundwork
- Memory Plane v2 persistence
- Tooling System v2 reliability features
- Swarm mesh libp2p integration
- Production hardening for runtime approval flow

## [1.1.0] - 2026-02-25

### Added
- GraphRAG + AST indexing
- Docker worker orchestration
- WASM tool sandbox
- Multi-provider LLM support

## [1.0.0] - 2026-02-19

### Added
- Initial release based on IronClaw from NEAR AI
- Multi-provider LLM runtime with failover
- Local-first memory with hybrid retrieval
- Secure WASM tool sandbox
- Web gateway with WebSocket + SSE
- Routines/automation engine