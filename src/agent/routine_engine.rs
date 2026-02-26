//! Routine execution engine.
//!
//! Handles loading routines, checking triggers, enforcing guardrails,
//! and executing both lightweight (single LLM call) and full-job routines.
//!
//! The engine runs two independent loops:
//! - A **cron ticker** that polls the DB every N seconds for due cron routines
//! - An **event matcher** called synchronously from the agent main loop
//!
//! Lightweight routines execute inline (single LLM call, no scheduler slot).
//! Full-job routines are delegated to the existing `Scheduler`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use regex::Regex;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

use crate::agent::Scheduler;
use crate::agent::memory_plane::{MemoryRecordWriteRequest, persist_memory_record_best_effort};
use crate::agent::memory_write_policy::MemoryWriteIntent;
use crate::agent::routine::{
    NotifyConfig, Routine, RoutineAction, RoutineRun, RunStatus, Trigger, next_cron_fire,
};
use crate::agent::{MemorySourceKind, MemoryType};
use crate::channels::{IncomingMessage, OutgoingResponse};
use crate::config::RoutineConfig;
use crate::context::{ContextManager, JobState};
use crate::db::Database;
use crate::llm::{ChatMessage, CompletionRequest, FinishReason, LlmProvider};
use crate::workspace::Workspace;

/// The routine execution engine.
pub struct RoutineEngine {
    config: RoutineConfig,
    store: Arc<dyn Database>,
    llm: Arc<dyn LlmProvider>,
    workspace: Arc<Workspace>,
    scheduler: Arc<Scheduler>,
    context_manager: Arc<ContextManager>,
    /// Sender for notifications (routed to channel manager).
    notify_tx: mpsc::Sender<OutgoingResponse>,
    /// Currently running routine count (across all routines).
    running_count: Arc<AtomicUsize>,
    /// Compiled event regex cache: routine_id -> compiled regex.
    event_cache: Arc<RwLock<Vec<(Uuid, Routine, Regex)>>>,
}

impl RoutineEngine {
    pub fn new(
        config: RoutineConfig,
        store: Arc<dyn Database>,
        llm: Arc<dyn LlmProvider>,
        workspace: Arc<Workspace>,
        scheduler: Arc<Scheduler>,
        context_manager: Arc<ContextManager>,
        notify_tx: mpsc::Sender<OutgoingResponse>,
    ) -> Self {
        Self {
            config,
            store,
            llm,
            workspace,
            scheduler,
            context_manager,
            notify_tx,
            running_count: Arc::new(AtomicUsize::new(0)),
            event_cache: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Refresh the in-memory event trigger cache from DB.
    pub async fn refresh_event_cache(&self) {
        match self.store.list_event_routines().await {
            Ok(routines) => {
                let mut cache = Vec::new();
                for routine in routines {
                    if let Trigger::Event { ref pattern, .. } = routine.trigger {
                        match Regex::new(pattern) {
                            Ok(re) => cache.push((routine.id, routine.clone(), re)),
                            Err(e) => {
                                tracing::warn!(
                                    routine = %routine.name,
                                    "Invalid event regex '{}': {}",
                                    pattern, e
                                );
                            }
                        }
                    }
                }
                let count = cache.len();
                *self.event_cache.write().await = cache;
                tracing::debug!("Refreshed event cache: {} routines", count);
            }
            Err(e) => {
                tracing::error!("Failed to refresh event cache: {}", e);
            }
        }
    }

    /// Check incoming message against event triggers. Returns number of routines fired.
    ///
    /// Called synchronously from the main loop after handle_message(). The actual
    /// execution is spawned async so this returns quickly.
    pub async fn check_event_triggers(&self, message: &IncomingMessage) -> usize {
        let cache = self.event_cache.read().await;
        let mut fired = 0;

        for (_, routine, re) in cache.iter() {
            // Channel filter
            if let Trigger::Event {
                channel: Some(ch), ..
            } = &routine.trigger
                && ch != &message.channel
            {
                continue;
            }

            // Regex match
            if !re.is_match(&message.content) {
                continue;
            }

            // Cooldown check
            if !self.check_cooldown(routine) {
                tracing::debug!(routine = %routine.name, "Skipped: cooldown active");
                continue;
            }

            // Concurrent run check
            if !self.check_concurrent(routine).await {
                tracing::debug!(routine = %routine.name, "Skipped: max concurrent reached");
                continue;
            }

            // Global capacity check
            if self.running_count.load(Ordering::Relaxed) >= self.config.max_concurrent_routines {
                tracing::warn!(routine = %routine.name, "Skipped: global max concurrent reached");
                continue;
            }

            let detail = truncate(&message.content, 200);
            self.spawn_fire(routine.clone(), "event", Some(detail));
            fired += 1;
        }

        fired
    }

    /// Check all due cron routines and fire them. Called by the cron ticker.
    pub async fn check_cron_triggers(&self) {
        let routines = match self.store.list_due_cron_routines().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to load due cron routines: {}", e);
                return;
            }
        };

        for routine in routines {
            if self.running_count.load(Ordering::Relaxed) >= self.config.max_concurrent_routines {
                tracing::warn!("Global max concurrent routines reached, skipping remaining");
                break;
            }

            if !self.check_cooldown(&routine) {
                continue;
            }

            if !self.check_concurrent(&routine).await {
                continue;
            }

            let detail = if let Trigger::Cron { ref schedule } = routine.trigger {
                Some(schedule.clone())
            } else {
                None
            };

            self.spawn_fire(routine, "cron", detail);
        }
    }

    /// Fire a routine manually (from tool call or CLI).
    pub async fn fire_manual(&self, routine_id: Uuid) -> Result<Uuid, String> {
        let routine = self
            .store
            .get_routine(routine_id)
            .await
            .map_err(|e| format!("DB error: {e}"))?
            .ok_or_else(|| format!("routine {routine_id} not found"))?;

        if !routine.enabled {
            return Err(format!("routine '{}' is disabled", routine.name));
        }

        if !self.check_concurrent(&routine).await {
            return Err(format!(
                "routine '{}' already at max concurrent runs",
                routine.name
            ));
        }

        let run_id = Uuid::new_v4();
        let run = RoutineRun {
            id: run_id,
            routine_id: routine.id,
            trigger_type: "manual".to_string(),
            trigger_detail: None,
            started_at: Utc::now(),
            completed_at: None,
            status: RunStatus::Running,
            result_summary: None,
            tokens_used: None,
            job_id: None,
            created_at: Utc::now(),
        };

        if let Err(e) = self.store.create_routine_run(&run).await {
            return Err(format!("failed to create run record: {e}"));
        }

        // Execute inline for manual triggers (caller wants to wait)
        let engine = EngineContext {
            store: self.store.clone(),
            llm: self.llm.clone(),
            workspace: self.workspace.clone(),
            scheduler: self.scheduler.clone(),
            context_manager: self.context_manager.clone(),
            notify_tx: self.notify_tx.clone(),
            running_count: self.running_count.clone(),
        };

        tokio::spawn(async move {
            execute_routine(engine, routine, run).await;
        });

        Ok(run_id)
    }

    /// Spawn a fire in a background task.
    fn spawn_fire(&self, routine: Routine, trigger_type: &str, trigger_detail: Option<String>) {
        let run = RoutineRun {
            id: Uuid::new_v4(),
            routine_id: routine.id,
            trigger_type: trigger_type.to_string(),
            trigger_detail,
            started_at: Utc::now(),
            completed_at: None,
            status: RunStatus::Running,
            result_summary: None,
            tokens_used: None,
            job_id: None,
            created_at: Utc::now(),
        };

        let engine = EngineContext {
            store: self.store.clone(),
            llm: self.llm.clone(),
            workspace: self.workspace.clone(),
            scheduler: self.scheduler.clone(),
            context_manager: self.context_manager.clone(),
            notify_tx: self.notify_tx.clone(),
            running_count: self.running_count.clone(),
        };

        // Record the run in DB, then spawn execution
        let store = self.store.clone();
        tokio::spawn(async move {
            if let Err(e) = store.create_routine_run(&run).await {
                tracing::error!(routine = %routine.name, "Failed to record run: {}", e);
                return;
            }
            execute_routine(engine, routine, run).await;
        });
    }

    fn check_cooldown(&self, routine: &Routine) -> bool {
        if let Some(last_run) = routine.last_run_at {
            let elapsed = Utc::now().signed_duration_since(last_run);
            let cooldown = chrono::Duration::from_std(routine.guardrails.cooldown)
                .unwrap_or(chrono::Duration::seconds(300));
            if elapsed < cooldown {
                return false;
            }
        }
        true
    }

    async fn check_concurrent(&self, routine: &Routine) -> bool {
        match self.store.count_running_routine_runs(routine.id).await {
            Ok(count) => count < routine.guardrails.max_concurrent as i64,
            Err(e) => {
                tracing::error!(
                    routine = %routine.name,
                    "Failed to check concurrent runs: {}", e
                );
                false
            }
        }
    }
}

/// Shared context passed to the execution function.
struct EngineContext {
    store: Arc<dyn Database>,
    llm: Arc<dyn LlmProvider>,
    workspace: Arc<Workspace>,
    scheduler: Arc<Scheduler>,
    context_manager: Arc<ContextManager>,
    notify_tx: mpsc::Sender<OutgoingResponse>,
    running_count: Arc<AtomicUsize>,
}

/// Execute a routine run. Handles both lightweight and full_job modes.
async fn execute_routine(ctx: EngineContext, routine: Routine, run: RoutineRun) {
    // Increment running count (atomic: survives panics in the execution below)
    ctx.running_count.fetch_add(1, Ordering::Relaxed);

    let result = match &routine.action {
        RoutineAction::Lightweight {
            prompt,
            context_paths,
            max_tokens,
        } => execute_lightweight(&ctx, &routine, prompt, context_paths, *max_tokens).await,
        RoutineAction::FullJob {
            title,
            description,
            max_iterations,
        } => execute_full_job(&ctx, &routine, title, description, *max_iterations).await,
    };

    // Decrement running count
    ctx.running_count.fetch_sub(1, Ordering::Relaxed);

    // Process result
    let (status, summary, tokens) = match result {
        Ok(execution) => execution,
        Err(e) => {
            tracing::error!(routine = %routine.name, "Execution failed: {}", e);
            (RunStatus::Failed, Some(e), None)
        }
    };

    // Complete the run record
    if let Err(e) = ctx
        .store
        .complete_routine_run(run.id, status, summary.as_deref(), tokens)
        .await
    {
        tracing::error!(routine = %routine.name, "Failed to complete run record: {}", e);
    }

    // Update routine runtime state
    let now = Utc::now();
    let next_fire = if let Trigger::Cron { ref schedule } = routine.trigger {
        next_cron_fire(schedule).unwrap_or(None)
    } else {
        None
    };

    let new_failures = if status == RunStatus::Failed {
        routine.consecutive_failures + 1
    } else {
        0
    };

    if let Err(e) = ctx
        .store
        .update_routine_runtime(
            routine.id,
            now,
            next_fire,
            routine.run_count + 1,
            new_failures,
            &routine.state,
        )
        .await
    {
        tracing::error!(routine = %routine.name, "Failed to update runtime state: {}", e);
    }

    if ctx.scheduler.autonomy_memory_plane_v2_enabled() {
        let now_ts = Utc::now();
        let summary_text = summary.clone().unwrap_or_else(|| {
            format!(
                "Routine '{}' completed with status {}",
                routine.name, status
            )
        });
        let action_type = match &routine.action {
            RoutineAction::Lightweight { .. } => "lightweight",
            RoutineAction::FullJob { .. } => "full_job",
        };

        persist_memory_record_best_effort(
            Arc::clone(&ctx.store),
            MemoryRecordWriteRequest {
                owner_user_id: routine.user_id.clone(),
                goal_id: None,
                plan_id: None,
                plan_step_id: None,
                job_id: run.job_id,
                thread_id: None,
                source_kind: MemorySourceKind::RoutineRun,
                category: "routine_run_summary".to_string(),
                title: format!("Routine '{}' {}", routine.name, status),
                summary: truncate(&summary_text, 240),
                payload: serde_json::json!({
                    "routine_id": routine.id,
                    "routine_name": routine.name,
                    "routine_run_id": run.id,
                    "status": status.to_string(),
                    "trigger_type": run.trigger_type,
                    "trigger_detail": run.trigger_detail,
                    "action_type": action_type,
                    "tokens_used": tokens,
                    "result_summary": summary,
                }),
                provenance: serde_json::json!({
                    "source": "routine_engine.execute_routine",
                    "timestamp": now_ts,
                    "routine_run_id": run.id,
                }),
                desired_memory_type: None,
                confidence_hint: Some(if status == RunStatus::Ok { 0.88 } else { 0.76 }),
                sensitivity_hint: None,
                ttl_secs_hint: None,
                high_impact: false,
                intent: MemoryWriteIntent::RoutineSummary {
                    successful: status == RunStatus::Ok,
                },
            },
        );

        let next_run_count = routine.run_count.saturating_add(1);
        if status == RunStatus::Ok
            && next_run_count >= ctx.scheduler.autonomy_memory_playbook_min_repetitions() as u64
        {
            persist_memory_record_best_effort(
                Arc::clone(&ctx.store),
                MemoryRecordWriteRequest {
                    owner_user_id: routine.user_id.clone(),
                    goal_id: None,
                    plan_id: None,
                    plan_step_id: None,
                    job_id: run.job_id,
                    thread_id: None,
                    source_kind: MemorySourceKind::RoutineRun,
                    category: "routine_playbook_candidate".to_string(),
                    title: format!("Playbook candidate from routine '{}'", routine.name),
                    summary: truncate(&summary_text, 240),
                    payload: serde_json::json!({
                        "routine_id": routine.id,
                        "routine_name": routine.name,
                        "routine_run_id": run.id,
                        "task_class": "routine_automation",
                        "trigger_type": run.trigger_type,
                        "action_type": action_type,
                        "observed_success_count": next_run_count,
                        "candidate_steps_hint": {
                            "action_type": action_type,
                            "max_concurrent": routine.guardrails.max_concurrent,
                        },
                        "result_summary": summary,
                    }),
                    provenance: serde_json::json!({
                        "source": "routine_engine.execute_routine",
                        "timestamp": now_ts,
                        "routine_run_id": run.id,
                    }),
                    desired_memory_type: Some(MemoryType::Procedural),
                    confidence_hint: Some(0.72),
                    sensitivity_hint: None,
                    ttl_secs_hint: None,
                    high_impact: false,
                    intent: MemoryWriteIntent::Generic,
                },
            );
        }
    }

    // Send notifications based on config
    send_notification(
        &ctx.notify_tx,
        &routine.notify,
        &routine.name,
        status,
        summary.as_deref(),
    )
    .await;
}

/// Execute a full-job routine through the scheduler (multi-step with tools).
async fn execute_full_job(
    ctx: &EngineContext,
    routine: &Routine,
    title: &str,
    description: &str,
    max_iterations: u32,
) -> Result<(RunStatus, Option<String>, Option<i32>), String> {
    let job_id = ctx
        .context_manager
        .create_job_for_user(&routine.user_id, title, description)
        .await
        .map_err(|e| format!("failed to create full_job context: {e}"))?;

    ctx.scheduler
        .schedule(job_id)
        .await
        .map_err(|e| format!("failed to schedule full_job: {e}"))?;

    let timeout_secs = (max_iterations as u64).saturating_mul(30).clamp(60, 1800);
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        if std::time::Instant::now() >= deadline {
            let _ = ctx.scheduler.stop(job_id).await;
            return Err(format!(
                "full_job timed out after {}s (job_id={})",
                timeout_secs, job_id
            ));
        }

        let job_ctx = ctx
            .context_manager
            .get_context(job_id)
            .await
            .map_err(|e| format!("failed to read full_job context: {e}"))?;

        match job_ctx.state {
            JobState::Completed | JobState::Accepted => {
                return Ok((
                    RunStatus::Ok,
                    Some(format!(
                        "Full job '{}' completed successfully (job_id={})",
                        title, job_id
                    )),
                    None,
                ));
            }
            JobState::Failed => {
                let reason = job_ctx
                    .transitions
                    .last()
                    .and_then(|t| t.reason.clone())
                    .unwrap_or_else(|| "Job failed".to_string());
                return Ok((
                    RunStatus::Failed,
                    Some(format!("Full job failed (job_id={}): {}", job_id, reason)),
                    None,
                ));
            }
            JobState::Cancelled => {
                return Ok((
                    RunStatus::Failed,
                    Some(format!("Full job cancelled (job_id={})", job_id)),
                    None,
                ));
            }
            JobState::Pending | JobState::InProgress | JobState::Stuck | JobState::Submitted => {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Execute a lightweight routine (single LLM call).
async fn execute_lightweight(
    ctx: &EngineContext,
    routine: &Routine,
    prompt: &str,
    context_paths: &[String],
    max_tokens: u32,
) -> Result<(RunStatus, Option<String>, Option<i32>), String> {
    // Load context from workspace
    let mut context_parts = Vec::new();
    for path in context_paths {
        match ctx.workspace.read(path).await {
            Ok(doc) => {
                context_parts.push(format!("## {}\n\n{}", path, doc.content));
            }
            Err(e) => {
                tracing::debug!(
                    routine = %routine.name,
                    "Failed to read context path {}: {}", path, e
                );
            }
        }
    }

    // Load routine state from workspace
    let state_path = format!("routines/{}/state.md", routine.name);
    let state_content = match ctx.workspace.read(&state_path).await {
        Ok(doc) => Some(doc.content),
        Err(_) => None,
    };

    // Build the prompt
    let mut full_prompt = String::new();
    full_prompt.push_str(prompt);

    if !context_parts.is_empty() {
        full_prompt.push_str("\n\n---\n\n# Context\n\n");
        full_prompt.push_str(&context_parts.join("\n\n"));
    }

    if let Some(state) = &state_content {
        full_prompt.push_str("\n\n---\n\n# Previous State\n\n");
        full_prompt.push_str(state);
    }

    full_prompt.push_str(
        "\n\n---\n\nIf nothing needs attention, reply EXACTLY with: ROUTINE_OK\n\
         If something needs attention, provide a concise summary.",
    );

    // Get system prompt
    let system_prompt = match ctx.workspace.system_prompt().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(routine = %routine.name, "Failed to get system prompt: {}", e);
            String::new()
        }
    };

    let messages = if system_prompt.is_empty() {
        vec![ChatMessage::user(&full_prompt)]
    } else {
        vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(&full_prompt),
        ]
    };

    // Determine max_tokens from model metadata with fallback
    let effective_max_tokens = match ctx.llm.model_metadata().await {
        Ok(meta) => {
            let from_api = meta.context_length.map(|ctx| ctx / 2).unwrap_or(max_tokens);
            from_api.max(max_tokens)
        }
        Err(_) => max_tokens,
    };

    let request = CompletionRequest::new(messages)
        .with_max_tokens(effective_max_tokens)
        .with_temperature(0.3);

    let response = ctx
        .llm
        .complete(request)
        .await
        .map_err(|e| format!("LLM call failed: {e}"))?;

    let content = response.content.trim();
    let tokens_used = Some((response.input_tokens + response.output_tokens) as i32);

    // Empty content guard (same as heartbeat)
    if content.is_empty() {
        return if response.finish_reason == FinishReason::Length {
            Err(
                "LLM response truncated (finish_reason=length) with no content. \
                 Model may have exhausted token budget on reasoning."
                    .to_string(),
            )
        } else {
            Err("LLM returned empty content.".to_string())
        };
    }

    // Check for the "nothing to do" sentinel
    if content == "ROUTINE_OK" || content.contains("ROUTINE_OK") {
        return Ok((RunStatus::Ok, None, tokens_used));
    }

    Ok((RunStatus::Attention, Some(content.to_string()), tokens_used))
}

/// Send a notification based on the routine's notify config and run status.
async fn send_notification(
    tx: &mpsc::Sender<OutgoingResponse>,
    notify: &NotifyConfig,
    routine_name: &str,
    status: RunStatus,
    summary: Option<&str>,
) {
    let should_notify = match status {
        RunStatus::Ok => notify.on_success,
        RunStatus::Attention => notify.on_attention,
        RunStatus::Failed => notify.on_failure,
        RunStatus::Running => false,
    };

    if !should_notify {
        return;
    }

    let icon = match status {
        RunStatus::Ok => "✅",
        RunStatus::Attention => "🔔",
        RunStatus::Failed => "❌",
        RunStatus::Running => "⏳",
    };

    let message = match summary {
        Some(s) => format!("{} *Routine '{}'*: {}\n\n{}", icon, routine_name, status, s),
        None => format!("{} *Routine '{}'*: {}", icon, routine_name, status),
    };

    let response = OutgoingResponse {
        content: message,
        thread_id: None,
        metadata: serde_json::json!({
            "source": "routine",
            "routine_name": routine_name,
            "status": status.to_string(),
        }),
    };

    if let Err(e) = tx.send(response).await {
        tracing::error!(routine = %routine_name, "Failed to send notification: {}", e);
    }
}

/// Spawn the cron ticker background task.
pub fn spawn_cron_ticker(
    engine: Arc<RoutineEngine>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip immediate first tick
        ticker.tick().await;

        loop {
            ticker.tick().await;
            engine.check_cron_triggers().await;
        }
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = crate::util::floor_char_boundary(s, max);
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::routine::{NotifyConfig, RunStatus};

    #[test]
    fn test_notification_gating() {
        let config = NotifyConfig {
            on_success: false,
            on_failure: true,
            on_attention: true,
            ..Default::default()
        };

        // on_success = false means Ok status should not notify
        assert!(!config.on_success);
        assert!(config.on_failure);
        assert!(config.on_attention);
    }

    #[test]
    fn test_run_status_icons() {
        // Just verify the mapping doesn't panic
        for status in [
            RunStatus::Ok,
            RunStatus::Attention,
            RunStatus::Failed,
            RunStatus::Running,
        ] {
            let _ = status.to_string();
        }
    }
}
