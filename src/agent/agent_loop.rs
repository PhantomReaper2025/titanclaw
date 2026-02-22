//! Main agent loop.
//!
//! Contains the `Agent` struct, `AgentDeps`, and the core event loop (`run`).
//! The heavy lifting is delegated to sibling modules:
//!
//! - `dispatcher` - Tool dispatch (agentic loop, tool execution)
//! - `commands` - System commands and job handlers
//! - `thread_ops` - Thread/session operations (user input, undo, approval, persistence)

use std::sync::Arc;

use futures::StreamExt;

use crate::agent::context_monitor::ContextMonitor;
use crate::agent::heartbeat::spawn_heartbeat;
use crate::agent::routine_engine::{RoutineEngine, spawn_cron_ticker};
use crate::agent::self_repair::{DefaultSelfRepair, RepairResult, SelfRepair};
use crate::agent::session_manager::SessionManager;
use crate::agent::shadow_engine::ShadowEngine;
use crate::agent::submission::{Submission, SubmissionParser, SubmissionResult};
use crate::agent::{HeartbeatConfig as AgentHeartbeatConfig, Router, Scheduler};
use crate::channels::{ChannelManager, IncomingMessage, OutgoingResponse, StatusUpdate};
use crate::config::{AgentConfig, HeartbeatConfig, RoutineConfig, SkillsConfig};
use crate::context::ContextManager;
use crate::db::Database;
use crate::error::Error;
use crate::extensions::ExtensionManager;
use crate::hooks::HookRegistry;
use crate::llm::LlmProvider;
use crate::orchestrator::job_manager::ContainerJobManager;
use crate::safety::SafetyLayer;
use crate::skills::SkillRegistry;
use crate::swarm::node::{SwarmHandle, SwarmResultRouter};
use crate::tools::ToolRegistry;
use crate::workspace::Workspace;

/// Collapse a tool output string into a single-line preview for display.
pub(crate) fn truncate_for_preview(output: &str, max_chars: usize) -> String {
    let collapsed: String = output
        .chars()
        .take(max_chars + 50)
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // char_indices gives us byte offsets at char boundaries, so the slice is always valid UTF-8.
    if collapsed.chars().count() > max_chars {
        let byte_offset = collapsed
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(collapsed.len());
        format!("{}...", &collapsed[..byte_offset])
    } else {
        collapsed
    }
}

/// Core dependencies for the agent.
///
/// Bundles the shared components to reduce argument count.
pub struct AgentDeps {
    pub store: Option<Arc<dyn Database>>,
    pub llm: Arc<dyn LlmProvider>,
    /// Cheap/fast LLM for lightweight tasks (heartbeat, routing, evaluation).
    /// Falls back to the main `llm` if None.
    pub cheap_llm: Option<Arc<dyn LlmProvider>>,
    pub safety: Arc<SafetyLayer>,
    pub tools: Arc<ToolRegistry>,
    pub workspace: Option<Arc<Workspace>>,
    pub extension_manager: Option<Arc<ExtensionManager>>,
    pub skill_registry: Option<Arc<std::sync::RwLock<SkillRegistry>>>,
    pub skills_config: SkillsConfig,
    pub hooks: Arc<HookRegistry>,
    /// Cost enforcement guardrails (daily budget, hourly rate limits).
    pub cost_guard: Arc<crate::agent::cost_guard::CostGuard>,
    /// Optional distributed swarm handle for remote task offload.
    pub swarm_handle: Option<SwarmHandle>,
    /// Optional swarm result router used to await remote task completions.
    pub swarm_results: Option<SwarmResultRouter>,
    /// Optional container job manager for sandbox lifecycle control.
    pub container_job_manager: Option<Arc<ContainerJobManager>>,
    /// True when launched in one-shot mode (`run -m`).
    pub single_message_mode: bool,
    /// Optional kernel monitor/orchestration engine.
    pub kernel_orchestrator: Option<Arc<crate::agent::kernel_orchestrator::KernelOrchestrator>>,
    /// Optional speculative shadow-worker engine.
    pub shadow_engine: Option<Arc<ShadowEngine>>,
}

/// The main agent that coordinates all components.
pub struct Agent {
    pub(super) config: AgentConfig,
    pub(super) deps: AgentDeps,
    pub(super) channels: Arc<ChannelManager>,
    pub(super) context_manager: Arc<ContextManager>,
    pub(super) scheduler: Arc<Scheduler>,
    pub(super) router: Router,
    pub(super) session_manager: Arc<SessionManager>,
    pub(super) context_monitor: ContextMonitor,
    pub(super) heartbeat_config: Option<HeartbeatConfig>,
    pub(super) routine_config: Option<RoutineConfig>,
    pub(super) kernel_orchestrator:
        Option<Arc<crate::agent::kernel_orchestrator::KernelOrchestrator>>,
    pub(super) shadow_engine: Option<Arc<ShadowEngine>>,
}

impl Agent {
    /// Create a new agent.
    ///
    /// Optionally accepts pre-created `ContextManager` and `SessionManager` for sharing
    /// with external components (job tools, web gateway). Creates new ones if not provided.
    pub fn new(
        config: AgentConfig,
        deps: AgentDeps,
        channels: ChannelManager,
        heartbeat_config: Option<HeartbeatConfig>,
        routine_config: Option<RoutineConfig>,
        context_manager: Option<Arc<ContextManager>>,
        session_manager: Option<Arc<SessionManager>>,
    ) -> Self {
        let kernel_orchestrator = deps.kernel_orchestrator.clone();
        let shadow_engine = deps.shadow_engine.clone();
        let context_manager = context_manager
            .unwrap_or_else(|| Arc::new(ContextManager::new(config.max_parallel_jobs)));

        let session_manager = session_manager.unwrap_or_else(|| Arc::new(SessionManager::new()));

        let scheduler = Arc::new(Scheduler::new(
            config.clone(),
            context_manager.clone(),
            deps.llm.clone(),
            deps.safety.clone(),
            deps.tools.clone(),
            deps.store.clone(),
            deps.hooks.clone(),
            deps.swarm_handle.clone(),
            deps.swarm_results.clone(),
        ));

        Self {
            config,
            deps,
            channels: Arc::new(channels),
            context_manager,
            scheduler,
            router: Router::new(),
            session_manager,
            context_monitor: ContextMonitor::new(),
            heartbeat_config,
            routine_config,
            kernel_orchestrator,
            shadow_engine,
        }
    }

    // Convenience accessors

    pub(super) fn store(&self) -> Option<&Arc<dyn Database>> {
        self.deps.store.as_ref()
    }

    pub(super) fn llm(&self) -> &Arc<dyn LlmProvider> {
        &self.deps.llm
    }

    /// Get the cheap/fast LLM provider, falling back to the main one.
    pub(super) fn cheap_llm(&self) -> &Arc<dyn LlmProvider> {
        self.deps.cheap_llm.as_ref().unwrap_or(&self.deps.llm)
    }

    pub(super) fn safety(&self) -> &Arc<SafetyLayer> {
        &self.deps.safety
    }

    pub(super) fn tools(&self) -> &Arc<ToolRegistry> {
        &self.deps.tools
    }

    pub(super) fn workspace(&self) -> Option<&Arc<Workspace>> {
        self.deps.workspace.as_ref()
    }

    pub(super) fn hooks(&self) -> &Arc<HookRegistry> {
        &self.deps.hooks
    }

    pub(super) fn cost_guard(&self) -> &Arc<crate::agent::cost_guard::CostGuard> {
        &self.deps.cost_guard
    }

    pub(super) fn skill_registry(&self) -> Option<&Arc<std::sync::RwLock<SkillRegistry>>> {
        self.deps.skill_registry.as_ref()
    }

    pub(super) fn kernel_orchestrator(
        &self,
    ) -> Option<&Arc<crate::agent::kernel_orchestrator::KernelOrchestrator>> {
        self.kernel_orchestrator.as_ref()
    }

    pub(super) fn shadow_engine(&self) -> Option<&Arc<ShadowEngine>> {
        self.shadow_engine.as_ref()
    }

    /// Select active skills for a message using deterministic prefiltering.
    pub(super) fn select_active_skills(
        &self,
        message_content: &str,
    ) -> Vec<crate::skills::LoadedSkill> {
        if let Some(registry) = self.skill_registry() {
            let guard = match registry.read() {
                Ok(g) => g,
                Err(e) => {
                    tracing::error!("Skill registry lock poisoned: {}", e);
                    return vec![];
                }
            };
            let available = guard.skills();
            let skills_cfg = &self.deps.skills_config;
            let selected = crate::skills::prefilter_skills(
                message_content,
                available,
                skills_cfg.max_active_skills,
                skills_cfg.max_context_tokens,
            );

            if !selected.is_empty() {
                tracing::debug!(
                    "Selected {} skill(s) for message: {}",
                    selected.len(),
                    selected
                        .iter()
                        .map(|s| s.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            selected.into_iter().cloned().collect()
        } else {
            vec![]
        }
    }

    /// Run the agent main loop.
    pub async fn run(self) -> Result<(), Error> {
        // Start channels
        let mut message_stream = self.channels.start_all().await?;

        // Start self-repair task with notification forwarding
        let repair = Arc::new(DefaultSelfRepair::new(
            self.context_manager.clone(),
            self.config.stuck_threshold,
            self.config.max_repair_attempts,
        ));
        let repair_interval = self.config.repair_check_interval;
        let repair_channels = self.channels.clone();
        let repair_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(repair_interval).await;

                // Check stuck jobs
                let stuck_jobs = repair.detect_stuck_jobs().await;
                for job in stuck_jobs {
                    tracing::info!("Attempting to repair stuck job {}", job.job_id);
                    let result = repair.repair_stuck_job(&job).await;
                    let notification = match &result {
                        Ok(RepairResult::Success { message }) => {
                            tracing::info!("Repair succeeded: {}", message);
                            Some(format!(
                                "Job {} was stuck for {}s, recovery succeeded: {}",
                                job.job_id,
                                job.stuck_duration.as_secs(),
                                message
                            ))
                        }
                        Ok(RepairResult::Failed { message }) => {
                            tracing::error!("Repair failed: {}", message);
                            Some(format!(
                                "Job {} was stuck for {}s, recovery failed permanently: {}",
                                job.job_id,
                                job.stuck_duration.as_secs(),
                                message
                            ))
                        }
                        Ok(RepairResult::ManualRequired { message }) => {
                            tracing::warn!("Manual intervention needed: {}", message);
                            Some(format!(
                                "Job {} needs manual intervention: {}",
                                job.job_id, message
                            ))
                        }
                        Ok(RepairResult::Retry { message }) => {
                            tracing::warn!("Repair needs retry: {}", message);
                            None // Don't spam the user on retries
                        }
                        Err(e) => {
                            tracing::error!("Repair error: {}", e);
                            None
                        }
                    };

                    if let Some(msg) = notification {
                        let response = OutgoingResponse::text(format!("Self-Repair: {}", msg));
                        let _ = repair_channels.broadcast_all("default", response).await;
                    }
                }

                // Check broken tools
                let broken_tools = repair.detect_broken_tools().await;
                for tool in broken_tools {
                    tracing::info!("Attempting to repair broken tool: {}", tool.name);
                    match repair.repair_broken_tool(&tool).await {
                        Ok(RepairResult::Success { message }) => {
                            let response = OutgoingResponse::text(format!(
                                "Self-Repair: Tool '{}' repaired: {}",
                                tool.name, message
                            ));
                            let _ = repair_channels.broadcast_all("default", response).await;
                        }
                        Ok(result) => {
                            tracing::info!("Tool repair result: {:?}", result);
                        }
                        Err(e) => {
                            tracing::error!("Tool repair error: {}", e);
                        }
                    }
                }
            }
        });

        let kernel_handle = if let Some(orchestrator) = self.kernel_orchestrator.clone() {
            if orchestrator.enabled() {
                let interval = orchestrator.interval();
                Some(tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(interval).await;
                        orchestrator.tick().await;
                    }
                }))
            } else {
                None
            }
        } else {
            None
        };

        let shadow_prune_handle = if let Some(shadow) = self.shadow_engine.clone() {
            if shadow.enabled() {
                Some(tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        shadow.prune_expired().await;
                    }
                }))
            } else {
                None
            }
        } else {
            None
        };

        // Spawn session pruning task
        let session_mgr = self.session_manager.clone();
        let session_idle_timeout = self.config.session_idle_timeout;
        let pruning_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(600)); // Every 10 min
            interval.tick().await; // Skip immediate first tick
            loop {
                interval.tick().await;
                session_mgr.prune_stale_sessions(session_idle_timeout).await;
            }
        });

        // Spawn heartbeat if enabled
        let heartbeat_handle = if let Some(ref hb_config) = self.heartbeat_config {
            if hb_config.enabled {
                if let Some(workspace) = self.workspace() {
                    let config = AgentHeartbeatConfig::default()
                        .with_interval(std::time::Duration::from_secs(hb_config.interval_secs));

                    // Set up notification channel
                    let (notify_tx, mut notify_rx) =
                        tokio::sync::mpsc::channel::<OutgoingResponse>(16);

                    // Spawn notification forwarder that routes through channel manager
                    let notify_channel = hb_config.notify_channel.clone();
                    let notify_user = hb_config.notify_user.clone();
                    let channels = self.channels.clone();
                    tokio::spawn(async move {
                        while let Some(response) = notify_rx.recv().await {
                            let user = notify_user.as_deref().unwrap_or("default");

                            // Try the configured channel first, fall back to
                            // broadcasting on all channels.
                            let targeted_ok = if let Some(ref channel) = notify_channel {
                                channels
                                    .broadcast(channel, user, response.clone())
                                    .await
                                    .is_ok()
                            } else {
                                false
                            };

                            if !targeted_ok {
                                let results = channels.broadcast_all(user, response).await;
                                for (ch, result) in results {
                                    if let Err(e) = result {
                                        tracing::warn!(
                                            "Failed to broadcast heartbeat to {}: {}",
                                            ch,
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    });

                    tracing::info!(
                        "Heartbeat enabled with {}s interval",
                        hb_config.interval_secs
                    );
                    Some(spawn_heartbeat(
                        config,
                        workspace.clone(),
                        self.cheap_llm().clone(),
                        Some(notify_tx),
                    ))
                } else {
                    tracing::warn!("Heartbeat enabled but no workspace available");
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Spawn routine engine if enabled
        let routine_handle = if let Some(ref rt_config) = self.routine_config {
            if rt_config.enabled {
                if let (Some(store), Some(workspace)) = (self.store(), self.workspace()) {
                    // Set up notification channel (same pattern as heartbeat)
                    let (notify_tx, mut notify_rx) =
                        tokio::sync::mpsc::channel::<OutgoingResponse>(32);

                    let engine = Arc::new(RoutineEngine::new(
                        rt_config.clone(),
                        Arc::clone(store),
                        self.llm().clone(),
                        Arc::clone(workspace),
                        Arc::clone(&self.scheduler),
                        Arc::clone(&self.context_manager),
                        notify_tx,
                    ));

                    // Register routine tools
                    self.deps
                        .tools
                        .register_routine_tools(Arc::clone(store), Arc::clone(&engine));

                    // Load initial event cache
                    engine.refresh_event_cache().await;

                    // Spawn notification forwarder
                    let channels = self.channels.clone();
                    tokio::spawn(async move {
                        while let Some(response) = notify_rx.recv().await {
                            let user = response
                                .metadata
                                .get("notify_user")
                                .and_then(|v| v.as_str())
                                .unwrap_or("default")
                                .to_string();
                            let results = channels.broadcast_all(&user, response).await;
                            for (ch, result) in results {
                                if let Err(e) = result {
                                    tracing::warn!(
                                        "Failed to broadcast routine notification to {}: {}",
                                        ch,
                                        e
                                    );
                                }
                            }
                        }
                    });

                    // Spawn cron ticker
                    let cron_interval =
                        std::time::Duration::from_secs(rt_config.cron_check_interval_secs);
                    let cron_handle = spawn_cron_ticker(Arc::clone(&engine), cron_interval);

                    // Store engine reference for event trigger checking
                    // Safety: we're in run() which takes self, no other reference exists
                    let engine_ref = Arc::clone(&engine);
                    // SAFETY: self is consumed by run(), we can smuggle the engine in
                    // via a local to use in the message loop below.

                    tracing::info!(
                        "Routines enabled: cron ticker every {}s, max {} concurrent",
                        rt_config.cron_check_interval_secs,
                        rt_config.max_concurrent_routines
                    );

                    Some((cron_handle, engine_ref))
                } else {
                    tracing::warn!("Routines enabled but store/workspace not available");
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Extract engine ref for use in message loop
        let routine_engine_for_loop = routine_handle.as_ref().map(|(_, e)| Arc::clone(e));

        // Spawn Reflex compiler background ticker
        let reflex_handle = if let Some(store) = self.store() {
            tracing::info!("Reflex compiler background ticker enabled (evaluating every 5m)");
            Some(crate::agent::reflex::spawn_reflex_compiler(
                Arc::clone(store),
                self.llm().clone(),
                self.safety().clone(),
                self.tools().clone(),
                std::time::Duration::from_secs(300),
            ))
        } else {
            None
        };

        // Main message loop
        tracing::info!("Agent {} ready and listening", self.config.name);

        loop {
            let message = tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Ctrl+C received, shutting down...");
                    break;
                }
                msg = message_stream.next() => {
                    match msg {
                        Some(m) => m,
                        None => {
                            tracing::info!("All channel streams ended, shutting down...");
                            break;
                        }
                    }
                }
            };

            match self.handle_message(&message).await {
                Ok(Some(response)) if !response.is_empty() => {
                    // Hook: BeforeOutbound — allow hooks to modify or suppress outbound
                    let event = crate::hooks::HookEvent::Outbound {
                        user_id: message.user_id.clone(),
                        channel: message.channel.clone(),
                        content: response.clone(),
                        thread_id: message.thread_id.clone(),
                    };
                    match self.hooks().run(&event).await {
                        Err(err) => {
                            tracing::warn!("BeforeOutbound hook blocked response: {}", err);
                        }
                        Ok(crate::hooks::HookOutcome::Continue {
                            modified: Some(new_content),
                        }) => {
                            let _ = self
                                .channels
                                .respond(&message, OutgoingResponse::text(new_content))
                                .await;
                        }
                        _ => {
                            let _ = self
                                .channels
                                .respond(&message, OutgoingResponse::text(response))
                                .await;
                        }
                    }
                }
                Ok(Some(_)) => {
                    // Empty response, nothing to send (e.g. approval handled via send_status)
                }
                Ok(None) => {
                    // Shutdown signal received (/quit, /exit, /shutdown)
                    tracing::info!("Shutdown command received, exiting...");
                    break;
                }
                Err(e) => {
                    tracing::error!("Error handling message: {}", e);
                    let _ = self
                        .channels
                        .respond(&message, OutgoingResponse::text(format!("Error: {}", e)))
                        .await;
                }
            }

            // Check event triggers (cheap in-memory regex, fires async if matched)
            if let Some(ref engine) = routine_engine_for_loop {
                let fired = engine.check_event_triggers(&message).await;
                if fired > 0 {
                    tracing::debug!("Fired {} event-triggered routines", fired);
                }
            }
        }

        if self.deps.single_message_mode {
            let timeout = std::time::Duration::from_secs(300);
            let started = std::time::Instant::now();
            let mut last_report_at = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(10))
                .unwrap_or_else(std::time::Instant::now);
            let mut last_snapshot: Option<(usize, usize)> = None;

            let _ = self
                .channels
                .send_status(
                    "repl",
                    StatusUpdate::Status(
                        "Single-message mode: waiting for spawned jobs...".to_string(),
                    ),
                    &serde_json::json!({}),
                )
                .await;

            loop {
                let active_context = self.context_manager.active_count().await;
                let active_containers = if let Some(jm) = &self.deps.container_job_manager {
                    let containers = jm.containers.read().await;
                    containers
                        .values()
                        .filter(|h| {
                            h.state == crate::orchestrator::job_manager::ContainerState::Creating
                                || h.state
                                    == crate::orchestrator::job_manager::ContainerState::Running
                        })
                        .count()
                } else {
                    0
                };

                if active_context == 0 && active_containers == 0 {
                    tracing::info!("Single-message mode: all spawned jobs have finished");
                    let _ = self
                        .channels
                        .send_status(
                            "repl",
                            StatusUpdate::Status(
                                "Single-message mode: all spawned jobs finished.".to_string(),
                            ),
                            &serde_json::json!({}),
                        )
                        .await;
                    break;
                }

                if started.elapsed() >= timeout {
                    tracing::warn!(
                        active_context,
                        active_containers,
                        "Single-message mode timeout while waiting for spawned jobs"
                    );
                    let _ = self
                        .channels
                        .send_status(
                            "repl",
                            StatusUpdate::Status(format!(
                                "Single-message mode timeout: {} context job(s), {} container job(s) still active.",
                                active_context, active_containers
                            )),
                            &serde_json::json!({}),
                        )
                        .await;
                    break;
                }

                tracing::info!(
                    active_context,
                    active_containers,
                    "Single-message mode waiting for spawned jobs"
                );
                let snapshot = (active_context, active_containers);
                if Some(snapshot) != last_snapshot
                    || last_report_at.elapsed() >= std::time::Duration::from_secs(10)
                {
                    last_snapshot = Some(snapshot);
                    last_report_at = std::time::Instant::now();
                    let _ = self
                        .channels
                        .send_status(
                            "repl",
                            StatusUpdate::Status(format!(
                                "Waiting: {} context job(s), {} container job(s) active...",
                                active_context, active_containers
                            )),
                            &serde_json::json!({}),
                        )
                        .await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }

        // Cleanup
        tracing::info!("Agent shutting down...");
        repair_handle.abort();
        pruning_handle.abort();
        if let Some(handle) = heartbeat_handle {
            handle.abort();
        }
        if let Some((cron_handle, _)) = routine_handle {
            cron_handle.abort();
        }
        if let Some(handle) = reflex_handle {
            handle.abort();
        }
        if let Some(handle) = kernel_handle {
            handle.abort();
        }
        if let Some(handle) = shadow_prune_handle {
            handle.abort();
        }
        self.scheduler.stop_all().await;
        self.channels.shutdown_all().await?;

        Ok(())
    }

    async fn handle_message(&self, message: &IncomingMessage) -> Result<Option<String>, Error> {
        // Parse submission type first
        let mut submission = SubmissionParser::parse(&message.content);

        // Hook: BeforeInbound — allow hooks to modify or reject user input
        if let Submission::UserInput { ref content } = submission {
            let event = crate::hooks::HookEvent::Inbound {
                user_id: message.user_id.clone(),
                channel: message.channel.clone(),
                content: content.clone(),
                thread_id: message.thread_id.clone(),
            };
            match self.hooks().run(&event).await {
                Err(crate::hooks::HookError::Rejected { reason }) => {
                    return Ok(Some(format!("[Message rejected: {}]", reason)));
                }
                Err(err) => {
                    return Ok(Some(format!("[Message blocked by hook policy: {}]", err)));
                }
                Ok(crate::hooks::HookOutcome::Continue {
                    modified: Some(new_content),
                }) => {
                    submission = Submission::UserInput {
                        content: new_content,
                    };
                }
                _ => {} // Continue, fail-open errors already logged in registry
            }
        }

        // Hydrate thread from DB if it's a historical thread not in memory
        if let Some(ref external_thread_id) = message.thread_id {
            self.maybe_hydrate_thread(message, external_thread_id).await;
        }

        // Resolve session and thread
        let (session, thread_id) = self
            .session_manager
            .resolve_thread(
                &message.user_id,
                &message.channel,
                message.thread_id.as_deref(),
            )
            .await;

        // Auth mode interception: if the thread is awaiting a token, route
        // the message directly to the credential store. Nothing touches
        // logs, turns, history, or compaction.
        let pending_auth = {
            let sess = session.lock().await;
            sess.threads
                .get(&thread_id)
                .and_then(|t| t.pending_auth.clone())
        };

        if let Some(pending) = pending_auth {
            match &submission {
                Submission::UserInput { content } => {
                    return self
                        .process_auth_token(message, &pending, content, session, thread_id)
                        .await;
                }
                _ => {
                    // Any control submission (interrupt, undo, etc.) cancels auth mode
                    let mut sess = session.lock().await;
                    if let Some(thread) = sess.threads.get_mut(&thread_id) {
                        thread.pending_auth = None;
                    }
                    // Fall through to normal handling
                }
            }
        }

        tracing::debug!(
            "Received message from {} on {} ({} chars)",
            message.user_id,
            message.channel,
            message.content.len()
        );

        // Process based on submission type
        let result = match submission {
            Submission::UserInput { content } => {
                self.process_user_input(message, session, thread_id, &content)
                    .await
            }
            Submission::SystemCommand { command, args } => {
                self.handle_system_command(&command, &args).await
            }
            Submission::Undo => self.process_undo(session, thread_id).await,
            Submission::Redo => self.process_redo(session, thread_id).await,
            Submission::Interrupt => self.process_interrupt(message, session, thread_id).await,
            Submission::Compact => self.process_compact(session, thread_id).await,
            Submission::Clear => self.process_clear(session, thread_id).await,
            Submission::NewThread => self.process_new_thread(message).await,
            Submission::Heartbeat => self.process_heartbeat().await,
            Submission::Summarize => self.process_summarize(session, thread_id).await,
            Submission::Suggest => self.process_suggest(session, thread_id).await,
            Submission::Quit => return Ok(None),
            Submission::SwitchThread { thread_id: target } => {
                self.process_switch_thread(message, target).await
            }
            Submission::Resume { checkpoint_id } => {
                self.process_resume(session, thread_id, checkpoint_id).await
            }
            Submission::ExecApproval {
                request_id,
                approved,
                always,
            } => {
                self.process_approval(
                    message,
                    session,
                    thread_id,
                    Some(request_id),
                    approved,
                    always,
                )
                .await
            }
            Submission::ApprovalResponse { approved, always } => {
                self.process_approval(message, session, thread_id, None, approved, always)
                    .await
            }
        };

        // Convert SubmissionResult to response string
        match result? {
            SubmissionResult::Response { content } => Ok(Some(content)),
            SubmissionResult::Ok { message } => Ok(message),
            SubmissionResult::Error { message } => Ok(Some(format!("Error: {}", message))),
            SubmissionResult::Interrupted => Ok(Some("Interrupted.".into())),
            SubmissionResult::NeedApproval {
                request_id,
                tool_name,
                description,
                parameters,
            } => {
                // Each channel renders the approval prompt via send_status.
                // Web gateway shows an inline card, REPL prints a formatted prompt, etc.
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::ApprovalNeeded {
                            request_id: request_id.to_string(),
                            tool_name,
                            description,
                            parameters,
                        },
                        &message.metadata,
                    )
                    .await;

                // Empty string signals the caller to skip respond() (no duplicate text)
                Ok(Some(String::new()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_for_preview;

    #[test]
    fn test_truncate_short_input() {
        assert_eq!(truncate_for_preview("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_empty_input() {
        assert_eq!(truncate_for_preview("", 10), "");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate_for_preview("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_over_limit() {
        let result = truncate_for_preview("hello world, this is long", 10);
        assert!(result.ends_with("..."));
        // "hello worl" = 10 chars + "..."
        assert_eq!(result, "hello worl...");
    }

    #[test]
    fn test_truncate_collapses_newlines() {
        let result = truncate_for_preview("line1\nline2\nline3", 100);
        assert!(!result.contains('\n'));
        assert_eq!(result, "line1 line2 line3");
    }

    #[test]
    fn test_truncate_collapses_whitespace() {
        let result = truncate_for_preview("hello   world", 100);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_truncate_multibyte_utf8() {
        // Each emoji is 4 bytes. Truncating at char boundary must not panic.
        let input = "😀😁😂🤣😃😄😅😆😉😊";
        let result = truncate_for_preview(input, 5);
        assert!(result.ends_with("..."));
        // First 5 chars = 5 emoji
        assert_eq!(result, "😀😁😂🤣😃...");
    }

    #[test]
    fn test_truncate_cjk_characters() {
        // CJK chars are 3 bytes each in UTF-8.
        let input = "你好世界测试数据很长的字符串";
        let result = truncate_for_preview(input, 4);
        assert_eq!(result, "你好世界...");
    }

    #[test]
    fn test_truncate_mixed_multibyte_and_ascii() {
        let input = "hello 世界 foo";
        let result = truncate_for_preview(input, 8);
        // 'h','e','l','l','o',' ','世','界' = 8 chars
        assert_eq!(result, "hello 世界...");
    }
}
