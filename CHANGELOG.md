# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Autonomy Control Plane v1 persistence groundwork: versioned autonomy domain types, Postgres/libSQL goal/plan/plan-step/execution/policy/incident schema (`V11`-`V16` + libSQL mirror), and backend store CRUD implementations for the new autonomy tables.
- Read-only web gateway autonomy inspection endpoints for goals/plans (`GET /api/goals`, `GET /api/goals/{id}`, `GET /api/goals/{id}/plans`, `GET /api/plans?goal_id=...`, `GET /api/plans/{id}`), user-scoped to the authenticated gateway user.
- User-scoped web gateway create endpoints for autonomy goals/plans (`POST /api/goals`, `POST /api/plans`) with server-generated IDs/timestamps and goal-ownership checks before plan creation.
- CLI autonomy commands for goals/plans (`titanclaw goal|plan create/list/show`) with DB-backed create/read access and user-scoped ownership validation through goal records.
- User-scoped web gateway goal/plan status update endpoints (`POST /api/goals/{id}/status`, `POST /api/plans/{id}/status`) with ownership checks and DB-backed status transitions.
- CLI status update commands for autonomy goals/plans (`titanclaw goal set-status`, `titanclaw plan set-status`) with enum validation and user-scoped ownership checks.
- User-scoped plan-step web APIs for autonomy plan structure management (`GET/POST /api/plans/{id}/steps`, `GET /api/plan-steps/{id}`, `POST /api/plan-steps/{id}/status`) backed by new `PlanStore` step read helpers in both PostgreSQL and libSQL backends.
- CLI plan-step commands (`titanclaw plan-step create|list|show|set-status`) for local plan-step creation, inspection, and status updates with user-scoped ownership validation.
- User-scoped autonomy telemetry inspection endpoints (`GET /api/plans/{id}/executions`, `GET /api/goals/{id}/policy-decisions`) to inspect persisted runtime execution attempts and policy decisions from worker/dispatcher instrumentation.
- Atomic plan-step replacement support for replans: `POST /api/plans/{id}/steps/replace` and `titanclaw plan-step replace`, backed by transactional `replace_plan_steps_for_plan` implementations in PostgreSQL and libSQL.
- Plan revisioning actions: `POST /api/plans/{id}/replan` and `titanclaw plan replan` create the next plan revision from an existing plan with optional metadata overrides, optional superseding of the source plan, and optional source-step copying into the new revision.
- Goal/plan lifecycle convenience actions: web aliases (`POST /api/goals/{id}/complete`, `/abandon`, `POST /api/plans/{id}/complete`, `/supersede`) and matching CLI aliases (`titanclaw goal complete|abandon`, `titanclaw plan complete|supersede`) on top of the existing status-update APIs.

### Changed

- Worker planning and chat tool dispatch now best-effort persist internal autonomy records: worker-generated `ActionPlan`s create/update `Goal`/`Plan`/`PlanStep` records during planned execution, and dispatcher approval/tool-attempt telemetry is additionally mirrored into DB-backed autonomy policy/execution tables without changing the existing approval UX or tracing emitters.
- Job runtime context now carries optional autonomy linkage IDs (`goal_id` / `plan_id` / `plan_step_id`) in memory so worker/dispatcher paths can correlate records more consistently during execution.
- `agent_jobs` now persists optional autonomy linkage IDs (`autonomy_goal_id`, `autonomy_plan_id`, `autonomy_plan_step_id`) across PostgreSQL/libSQL (`V17` + libSQL schema compatibility path), so autonomy correlation survives DB save/load and restart boundaries.

## [1.0.2] - 2026-02-23

### Fixed

- Patch release retag on the latest `main` commit so published source includes the onboarding/profile synthesis/WASM introspection modules and compiles successfully (the earlier `v1.0.1` tag pointed to a prior commit).

## [1.0.1] - 2026-02-23

### Fixed

- Session/thread workflow reliability: gateway/web external UUID thread IDs are preserved during resolution (non-gateway channels stay channel-scoped), thread mapping creation is serialized to reduce duplicate-thread races, hydration avoids overwriting concurrently created in-memory threads, `register_thread` logs mapping collisions instead of silently overwriting, and chat thread delete/clear now cleans in-memory thread mappings + undo managers immediately instead of waiting for session pruning.
- Pending approval requests now expire automatically (timeout) and are canceled instead of leaving threads waiting indefinitely.
- Conversation turn persistence now retries DB writes (with backoff) and emits an in-channel warning if persistence still fails after retries, reducing silent chat-history loss.
- Approval rejection/resume paths now finalize and persist the current turn consistently, and runtime turn starts use guarded state checks (`try_start_turn`) to avoid invalid state transitions.
- Scheduler subtask tracking no longer reports successful subtasks as errors during background task execution.
- Sandbox job tool now returns a structured execution error when sandbox dependencies/job manager are unavailable instead of panicking the agent process.
- WASM OAuth callback endpoint now requires and validates/consumes the `state` nonce before accepting remote auth callbacks.
- WASM channel onboarding validation endpoints are now executed (HTTP GET) with secret placeholder substitution, catching misconfigurations during setup instead of deferring failure to runtime.
- Embedding providers now enforce approximate character-length checks (not byte length) consistently for both single and batched embedding requests.
- WhatsApp channel no longer silently drops non-text inbound messages; unsupported message types are surfaced as explicit placeholder messages to the agent and logged as warnings.
- AST graph query error messaging now explicitly documents that PostgreSQL-backed workspaces are not yet supported for this feature.
- Web gateway SSE auth now accepts URL-encoded query tokens (for EventSource clients), and WebSocket Origin validation correctly parses localhost loopback hosts including IPv6.
- Jobs UI manual-create flow now sends JSON with the correct content type and surfaces backend error messages in the UI instead of generic HTTP status text.
- Sandbox worker completion detection now recognizes common completion phrasings (including repeated “Job Complete” style outputs), adds a completion-like repetition guard to stop terminal-state loops, and continues to terminate jobs via the structured `/worker/{job_id}/complete` report path.
- Manual `/compact` now snapshots and compacts outside the session mutex, then applies results only if the thread is unchanged, reducing chat stalls during compaction.
- Thread turn tool-call records now preserve tool-call IDs / sanitized tool-result text, and `Thread::messages()` rebuilds tool messages into LLM context.
- Swarm task broadcasts now honor task-level assignee targeting (`assignee_node`) and receiver-side duplicate task suppression, preventing repeated execution across peers after broadcast/replay.
- Critical background loops (self-repair, kernel orchestrator, shadow cache pruning, reflex compiler) now run under restartable supervisors with backoff and shutdown signaling instead of silently dying on panic/unexpected exit.

### Changed

- WASM tool loader now reads optional sidecar metadata/schema overrides (`description`, `schema`, `tool.{description,schema}`) and applies them at registration time; runtime fallback metadata remains but is warning-logged.
- NEAR AI session renewal menu no longer offers the unimplemented NEAR Wallet auth option (GitHub/Google only).
- WASM tool preparation now probes WIT exports (`description()` / `schema()`) to populate LLM-facing metadata before falling back to generic runtime metadata.
- Web chat markdown rendering now uses DOM-based allowlist sanitization and DOM-bound copy-button wiring (replacing regex sanitization and inline `onclick` handlers).
- Sandbox archive downloads now stream `tar` output with a timeout watchdog instead of buffering full archives in gateway memory.
- Swarm remote wait fallback timeout default increased from `300ms` to `2500ms` to reduce premature local fallback under normal network/tool latency.
- Swarm remote wait fallback timeout is now configurable via `SWARM_REMOTE_WAIT_TIMEOUT_MS` (clamped to a safe range) instead of being hardcoded.

### Added

- Background profile synthesis engine that batches successful turns asynchronously (debounced) and updates managed sections in workspace core docs (`AGENTS.md`, `IDENTITY.md`, `SOUL.md`, `USER.md`, `MEMORY.md`) while preserving manual content outside markers.
- New profile synthesis configuration knobs (`PROFILE_SYNTHESIS_*`) for enablement, debounce, batching, minimum turn size, and optional LLM merge.
- Conversational profile onboarding (OpenClaw-style) with a soft-block first-chat flow that asks identity/goals/tone/work-style/boundaries questions, supports `/onboard profile` commands (`status|defer|skip|reset`), and writes managed baseline sections to core docs after review + confirm.

## [1.0.0](https://github.com/PhantomReaper2025/titanclaw/compare/v0.6.3...v1.0.0) - 2026-02-22

### Added

- Web gateway chat lifecycle controls:
  - `DELETE /api/chat/thread/{id}` for single-thread hard delete
  - `DELETE /api/chat/threads` for clear-all hard delete (chat scope)
- Sandbox output archive export endpoint: `GET /api/jobs/{id}/files/download`
- Jobs UI actions for thread deletion, clear-all chats, and archive download
- Onboarding prompt for default sandbox coding runtime and OpenCode default model
- `memory bootstrap` CLI command for seeding and safe-refreshing core workspace docs (`--dry-run`, `--force`)
- Jobs tab manual creation flow (`POST /api/jobs`) with mode selection (`worker`, `claude_code`, `opencode`)

### Changed

- `create_job` sandbox mode now accepts `opencode` in addition to `worker` and `claude_code`
- `CODING_RUNTIME_DEFAULT` is now respected as the default sandbox mode when `mode` is omitted
- Workspace startup now runs conservative core-doc synchronization using managed template markers and legacy detection
- `JobMode::OpenCode` now launches a dedicated OpenCode bridge command (`opencode-bridge`) instead of silently falling back to the generic worker runtime
- `RoutineAction::FullJob` now executes via real scheduler job flow (context creation + `Scheduler::schedule`) instead of being downgraded to lightweight single-call execution

### Fixed

- Claude bridge failure reporting now includes recent stderr context to diagnose `claude exited with code 1` failures faster
- Worker max-iteration failure reason now includes actionable remediation guidance
- OpenCode model and runtime selection are now effective at execution time (bridge consumes mode/model wiring instead of ignored env-only fallback)
- Worker image now exposes `opencode` on PATH for sandbox user and pre-creates OpenCode writable directories

### Infrastructure

- `Dockerfile.worker` now installs OpenCode CLI by default (`curl -fsSL https://opencode.ai/install | bash`)

## [0.6.3](https://github.com/PhantomReaper2025/titanclaw/compare/v0.6.2...v0.6.3) - 2026-02-21

### Fixed

- OpenRouter attribution now sends `X-Title: TitanClaw` and `HTTP-Referer: https://github.com/PhantomReaper2025/titanclaw`, so OpenRouter logs no longer show app identity as unknown.
- Auto-onboarding in `run` now reloads `.env` + `~/.ironclaw/.env` in-process after wizard completion, preventing first-run missing-key states.
- OpenRouter auth validation now fails early with a clear configuration error when `LLM_API_KEY` is missing, avoiding delayed runtime 401 failures.
- API-key providers (OpenAI/Anthropic/OpenAI-compatible) now persist env fallback keys to `~/.ironclaw/.env` when encrypted secrets storage is unavailable.
- Onboarding now preflights sandbox image readiness and auto-recovers by building local fallback image `titanclaw-worker:latest` when registry pull fails.
- Onboarding now persists `SANDBOX_IMAGE` and `SANDBOX_AUTO_PULL` to bootstrap env so runtime job manager uses the same image selected/built during setup.
- Sandbox preflight now prefers configured `SANDBOX_IMAGE` first, preventing repeated attempts to pull `ghcr.io/nearai/sandbox:latest` after fallback image is already configured.
- `Dockerfile.worker` now builds and runs the correct `titanclaw` binary (instead of `ironclaw`), fixing worker-image fallback builds.
- Web gateway now has resilient SSE reconnect/backoff for both chat and logs streams, reducing cases that required manual page refresh.

## [0.6.2](https://github.com/PhantomReaper2025/titanclaw/compare/v0.6.1...v0.6.2) - 2026-02-21

### Fixed

- OpenRouter/OpenAI-compatible onboarding now persists `LLM_API_KEY` to `~/.ironclaw/.env` when OS secrets storage is unavailable, preventing first-run `401 Missing Authentication header` failures.
- Bootstrap env persistence now merges with existing `~/.ironclaw/.env` entries instead of overwriting them, preserving previously saved keys such as `SECRETS_MASTER_KEY` and `LLM_API_KEY`.
- Container job startup reliability remains hardened with missing-image auto-pull support (respects `sandbox.auto_pull_image`).

## [0.6.1](https://github.com/PhantomReaper2025/titanclaw/compare/v0.6.0...v0.6.1) - 2026-02-21

### Added

- Production-core swarm hardening:
  - capability-gated remote offload from scheduler
  - deterministic local fallback on timeout/failure
  - bounded remote waiter routing with expiry cleanup

### Changed

- Rebranded release package identity from `ironclaw` to `titanclaw`
- Updated package metadata links to the TitanClaw repository
- Updated benchmark dependency mapping to track the renamed package

### Fixed

- Regenerated WiX installer metadata to match `titanclaw` binary/product naming:
  - MSI product name and install path
  - executable filename/path (`titanclaw.exe`)
  - installer help link URL

## [0.6.0](https://github.com/nearai/ironclaw/compare/ironclaw-v0.5.0...ironclaw-v0.6.0) - 2026-02-19

### Added

- add issue triage skill ([#200](https://github.com/nearai/ironclaw/pull/200))
- add PR triage dashboard skill ([#196](https://github.com/nearai/ironclaw/pull/196))
- add OpenRouter usage examples ([#189](https://github.com/nearai/ironclaw/pull/189))
- add Tinfoil private inference provider ([#62](https://github.com/nearai/ironclaw/pull/62))
- shell env scrubbing and command injection detection ([#164](https://github.com/nearai/ironclaw/pull/164))
- Add PR review tools, job monitor, and channel injection for E2E sandbox workflows ([#57](https://github.com/nearai/ironclaw/pull/57))
- Secure prompt-based skills system (Phases 1-4) ([#51](https://github.com/nearai/ironclaw/pull/51))
- Add benchmarking harness with spot suite ([#10](https://github.com/nearai/ironclaw/pull/10))
- 10 infrastructure improvements from zeroclaw ([#126](https://github.com/nearai/ironclaw/pull/126))

### Fixed

- *(rig)* prevent OpenAI Responses API panic on tool call IDs ([#182](https://github.com/nearai/ironclaw/pull/182))
- *(docs)* correct settings storage path in README ([#194](https://github.com/nearai/ironclaw/pull/194))
- OpenAI tool calling — schema normalization, missing types, and Responses API panic ([#132](https://github.com/nearai/ironclaw/pull/132))
- *(security)* prevent path traversal bypass in WASM HTTP allowlist ([#137](https://github.com/nearai/ironclaw/pull/137))
- persist OpenAI-compatible provider and respect embeddings disable ([#177](https://github.com/nearai/ironclaw/pull/177))
- remove .expect() calls in FailoverProvider::try_providers ([#156](https://github.com/nearai/ironclaw/pull/156))
- sentinel value collision in FailoverProvider cooldown ([#125](https://github.com/nearai/ironclaw/pull/125)) ([#154](https://github.com/nearai/ironclaw/pull/154))
- skills module audit cleanup ([#173](https://github.com/nearai/ironclaw/pull/173))

### Other

- Fix division by zero panic in ValueEstimator::is_profitable ([#139](https://github.com/nearai/ironclaw/pull/139))
- audit feature parity matrix against codebase and recent commits ([#202](https://github.com/nearai/ironclaw/pull/202))
- architecture improvements for contributor velocity ([#198](https://github.com/nearai/ironclaw/pull/198))
- fix rustfmt formatting from PR #137
- add .env.example examples for Ollama and OpenAI-compatible ([#110](https://github.com/nearai/ironclaw/pull/110))

## [0.5.0](https://github.com/nearai/ironclaw/compare/v0.4.0...v0.5.0) - 2026-02-17

### Added

- add cooldown management to FailoverProvider ([#114](https://github.com/nearai/ironclaw/pull/114))

## [0.4.0](https://github.com/nearai/ironclaw/compare/v0.3.0...v0.4.0) - 2026-02-17

### Added

- move per-invocation approval check into Tool trait ([#119](https://github.com/nearai/ironclaw/pull/119))
- add polished boot screen on CLI startup ([#118](https://github.com/nearai/ironclaw/pull/118))
- Add lifecycle hooks system with 6 interception points ([#18](https://github.com/nearai/ironclaw/pull/18))

### Other

- remove accidentally committed .sidecar and .todos directories ([#123](https://github.com/nearai/ironclaw/pull/123))

## [0.3.0](https://github.com/nearai/ironclaw/compare/v0.2.0...v0.3.0) - 2026-02-17

### Added

- direct api key and cheap model ([#116](https://github.com/nearai/ironclaw/pull/116))

## [0.2.0](https://github.com/nearai/ironclaw/compare/v0.1.3...v0.2.0) - 2026-02-16

### Added

- mark Ollama + OpenAI-compatible as implemented ([#102](https://github.com/nearai/ironclaw/pull/102))
- multi-provider inference + libSQL onboarding selection ([#92](https://github.com/nearai/ironclaw/pull/92))
- add multi-provider LLM failover with retry backoff ([#28](https://github.com/nearai/ironclaw/pull/28))
- add libSQL/Turso embedded database backend ([#47](https://github.com/nearai/ironclaw/pull/47))
- Move debug log truncation from agent loop to REPL channel ([#65](https://github.com/nearai/ironclaw/pull/65))

### Fixed

- shell destructive-command check bypassed by Value::Object arguments ([#72](https://github.com/nearai/ironclaw/pull/72))
- propagate real tool_call_id instead of hardcoded placeholder ([#73](https://github.com/nearai/ironclaw/pull/73))
- Fix wasm tool schemas and runtime ([#42](https://github.com/nearai/ironclaw/pull/42))
- flatten tool messages for NEAR AI cloud-api compatibility ([#41](https://github.com/nearai/ironclaw/pull/41))
- security hardening across all layers ([#35](https://github.com/nearai/ironclaw/pull/35))

### Other

- Explicitly enable cargo-dist caching for binary artifacts building
- Skip building binary artifacts on every PR
- add module specification rules to CLAUDE.md
- add setup/onboarding specification (src/setup/README.md)
- deduplicate tool code and remove dead stubs ([#98](https://github.com/nearai/ironclaw/pull/98))
- Reformat architecture diagram in README ([#64](https://github.com/nearai/ironclaw/pull/64))
- Add review discipline guidelines to CLAUDE.md ([#68](https://github.com/nearai/ironclaw/pull/68))
- Bump MSRV to 1.92, add GCP deployment files ([#40](https://github.com/nearai/ironclaw/pull/40))
- Add OpenAI-compatible HTTP API (/v1/chat/completions, /v1/models)   ([#31](https://github.com/nearai/ironclaw/pull/31))

## [0.1.3](https://github.com/nearai/ironclaw/compare/v0.1.2...v0.1.3) - 2026-02-12

### Other

- Enabled builds caching during CI/CD
- Disabled npm publishing as the name is already taken

## [0.1.2](https://github.com/nearai/ironclaw/compare/v0.1.1...v0.1.2) - 2026-02-12

### Other

- Added Installation instructions for the pre-built binaries
- Disabled Windows ARM64 builds as auto-updater [provided by cargo-dist] does not support this platform yet and it is not a common platform for us to support

## [0.1.1](https://github.com/nearai/ironclaw/compare/v0.1.0...v0.1.1) - 2026-02-12

### Other

- Renamed the secrets in release-plz.yml to match the configuration
- Make sure that the binaries release CD it kicking in after release-plz

## [0.1.0](https://github.com/nearai/ironclaw/releases/tag/v0.1.0) - 2026-02-12

### Added

- Add multi-provider LLM support via rig-core adapter ([#36](https://github.com/nearai/ironclaw/pull/36))
- Sandbox jobs ([#4](https://github.com/nearai/ironclaw/pull/4))
- Add Google Suite & Telegram WASM tools ([#9](https://github.com/nearai/ironclaw/pull/9))
- Improve CLI ([#5](https://github.com/nearai/ironclaw/pull/5))

### Fixed

- resolve runtime panic in Linux keychain integration ([#32](https://github.com/nearai/ironclaw/pull/32))

### Other

- Skip release-plz on forks
- Upgraded release-plz CD pipeline
- Added CI/CD and release pipelines ([#45](https://github.com/nearai/ironclaw/pull/45))
- DM pairing + Telegram channel improvements ([#17](https://github.com/nearai/ironclaw/pull/17))
- Fixes build, adds missing sse event and correct command ([#11](https://github.com/nearai/ironclaw/pull/11))
- Codex/feature parity pr hook ([#6](https://github.com/nearai/ironclaw/pull/6))
- Add WebSocket gateway and control plane ([#8](https://github.com/nearai/ironclaw/pull/8))
- select bundled Telegram channel and auto-install ([#3](https://github.com/nearai/ironclaw/pull/3))
- Adding skills for reusable work
- Fix MCP tool calls, approval loop, shutdown, and improve web UI
- Add auth mode, fix MCP token handling, and parallelize startup loading
- Merge remote-tracking branch 'origin/main' into ui
- Adding web UI
- Rename `setup` CLI command to `onboard` for compatibility
- Add in-chat extension discovery, auth, and activation system
- Add Telegram typing indicator via WIT on-status callback
- Add proactivity features: memory CLI, session pruning, self-repair notifications, slash commands, status diagnostics, context warnings
- Add hosted MCP server support with OAuth 2.1 and token refresh
- Add interactive setup wizard and persistent settings
- Rebrand to IronClaw with security-first mission
- Fix build_software tool stuck in planning mode loop
- Enable sandbox by default
- Fix Telegram Markdown formatting and clarify tool/memory distinctions
- Simplify Telegram channel config with host-injected tunnel/webhook settings
- Apply Telegram channel learnings to WhatsApp implementation
- Merge remote-tracking branch 'origin/main'
- Docker file for sandbox
- Replace hardcoded intent patterns with job tools
- Fix router test to match intentional job creation patterns
- Add Docker execution sandbox for secure shell command isolation
- Move setup wizard credentials to database storage
- Add interactive setup wizard for first-run configuration
- Add Telegram Bot API channel as WASM module
- Add OpenClaw feature parity tracking matrix
- Add Chat Completions API support and expand REPL debugging
- Implementing channels to be handled in wasm
- Support non interactive mode and model selection
- Implement tool approval, fix tool definition refresh, and wire embeddings
- Tool use
- Wiring more
- Add heartbeat integration, planning phase, and auto-repair
- Login flow
- Extend support for session management
- Adding builder capability
- Load tools at launch
- Fix multiline message rendering in TUI
- Parse NEAR AI alternative response format with output field
- Handle NEAR AI plain text responses
- Disable mouse capture to allow text selection in TUI
- Add verbose logging to debug empty NEAR AI responses
- Improve NEAR AI response parsing for varying response formats
- Show status/thinking messages in chat window, debug empty responses
- Add timeout and logging to NEAR AI provider
- Add status updates to show agent thinking/processing state
- Add CLI subcommands for WASM tool management
- Fix TUI shutdown: send /shutdown message and handle in agent loop
- Remove SimpleCliChannel, add Ctrl+D twice quit, redirect logs to TUI
- Fix TuiChannel integration and enable in main.rs
- Integrate Codex patterns: task scheduler, TUI, sessions, compaction
- Adding LICENSE
- Add README with IronClaw branding
- Add WASM sandbox secure API extension
- Wire database Store into agent loop
- Implementing WASM runtime
- Add workspace integration tests
- Compact memory_tree output format
- Replace memory_list with memory_tree tool
- Simplify workspace to path-based storage, remove legacy code
- Add NEAR AI chat-api as default LLM provider
- Add CLAUDE.md project documentation
- Add workspace and memory system (OpenClaw-inspired)
- Initial implementation of the agent framework
