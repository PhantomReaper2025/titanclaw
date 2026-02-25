//! Thread and session operations for the agent.
//!
//! Extracted from `agent_loop.rs` to isolate thread management (user input
//! processing, undo/redo, approval, auth, persistence) from the core loop.

use std::sync::Arc;

use chrono::Utc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent::Agent;
use crate::agent::PolicyDecision as AutonomyPolicyDecision;
use crate::agent::autonomy_telemetry::{
    PolicyDecisionKind as TelemetryPolicyDecisionKind, PolicyDecisionRecord, emit_policy_decision,
};
use crate::agent::compaction::ContextCompactor;
use crate::agent::context_monitor::CompactionStrategy;
use crate::agent::dispatcher::{AgenticLoopResult, detect_auth_awaiting, parse_auth_result};
use crate::agent::policy_engine::{
    ApprovalPolicyOutcome, HookToolPolicyOutcome, evaluate_dispatcher_tool_approval,
    evaluate_tool_call_hook, map_policy_decision_kind,
};
use crate::agent::session::{Session, Thread, ThreadState};
use crate::agent::submission::SubmissionResult;
use crate::channels::{IncomingMessage, StatusUpdate};
use crate::context::JobContext;
use crate::error::Error;
use crate::llm::ChatMessage;
use crate::workspace::Workspace;

const APPROVAL_TIMEOUT_SECS: i64 = 3600;

fn persist_chat_policy_decision_best_effort(
    agent: &Agent,
    message: &IncomingMessage,
    thread_id: Uuid,
    job_ctx: &JobContext,
    tool_name: &str,
    tool_call_id: &str,
    decision: TelemetryPolicyDecisionKind,
    reason_codes: Vec<String>,
    auto_approved: Option<bool>,
    action_kind: &str,
) {
    emit_policy_decision(&PolicyDecisionRecord {
        user_id: message.user_id.clone(),
        channel: message.channel.clone(),
        thread_id,
        tool_name: tool_name.to_string(),
        tool_call_id: tool_call_id.to_string(),
        decision,
        reason_codes: reason_codes.clone(),
        auto_approved,
    });

    let Some(store) = agent.store().cloned() else {
        return;
    };
    let record = AutonomyPolicyDecision {
        id: Uuid::new_v4(),
        goal_id: job_ctx.autonomy_goal_id,
        plan_id: job_ctx.autonomy_plan_id,
        plan_step_id: job_ctx.autonomy_plan_step_id,
        execution_attempt_id: None,
        user_id: message.user_id.clone(),
        channel: message.channel.clone(),
        tool_name: Some(tool_name.to_string()),
        tool_call_id: Some(tool_call_id.to_string()),
        action_kind: action_kind.to_string(),
        decision: map_policy_decision_kind(decision),
        reason_codes,
        risk_score: None,
        confidence: None,
        requires_approval: matches!(decision, TelemetryPolicyDecisionKind::RequireApproval),
        auto_approved,
        evidence_required: serde_json::json!({}),
        created_at: Utc::now(),
    };
    tokio::spawn(async move {
        if let Err(e) = store.record_policy_decision(&record).await {
            tracing::warn!(
                "Failed to persist autonomy policy decision from approval flow: {}",
                e
            );
        }
    });
}

fn normalize_reflex_pattern(input: &str) -> String {
    input
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl Agent {
    /// Hydrate a historical thread from DB into memory if not already present.
    ///
    /// Called before `resolve_thread` so that the session manager finds the
    /// thread on lookup instead of creating a new one.
    ///
    /// Creates an in-memory thread with the exact UUID the frontend sent,
    /// even when the conversation has zero messages (e.g. a brand-new
    /// assistant thread). Without this, `resolve_thread` would mint a
    /// fresh UUID and all messages would land in the wrong conversation.
    pub(super) async fn maybe_hydrate_thread(
        &self,
        message: &IncomingMessage,
        external_thread_id: &str,
    ) {
        // Only hydrate UUID-shaped thread IDs (web gateway uses UUIDs)
        let thread_uuid = match Uuid::parse_str(external_thread_id) {
            Ok(id) => id,
            Err(_) => return,
        };

        // Check if already in memory
        let session = self
            .session_manager
            .get_or_create_session(&message.user_id)
            .await;
        {
            let sess = session.lock().await;
            if sess.threads.contains_key(&thread_uuid) {
                return;
            }
        }

        // Load history from DB (may be empty for a newly created thread).
        let mut chat_messages: Vec<ChatMessage> = Vec::new();
        let msg_count;

        if let Some(store) = self.store() {
            let db_messages = store
                .list_conversation_messages(thread_uuid)
                .await
                .unwrap_or_default();
            msg_count = db_messages.len();
            chat_messages = db_messages
                .iter()
                .filter_map(|m| match m.role.as_str() {
                    "user" => Some(ChatMessage::user(&m.content)),
                    "assistant" => Some(ChatMessage::assistant(&m.content)),
                    _ => None,
                })
                .collect();
        } else {
            msg_count = 0;
        }

        // Create thread with the historical ID and restore messages
        let session_id = {
            let sess = session.lock().await;
            sess.id
        };

        let mut thread = crate::agent::session::Thread::with_id(thread_uuid, session_id);
        if !chat_messages.is_empty() {
            thread.restore_from_messages(chat_messages);
        }

        // Restore response chain from conversation metadata
        if let Some(store) = self.store()
            && let Ok(Some(metadata)) = store.get_conversation_metadata(thread_uuid).await
            && let Some(rid) = metadata
                .get("last_response_id")
                .and_then(|v| v.as_str())
                .map(String::from)
        {
            thread.last_response_id = Some(rid.clone());
            self.llm()
                .seed_response_chain(&thread_uuid.to_string(), rid);
            tracing::debug!("Restored response chain for thread {}", thread_uuid);
        }

        // Insert into session and register with session manager
        {
            let mut sess = session.lock().await;
            if sess.threads.contains_key(&thread_uuid) {
                tracing::debug!(
                    "Skipped hydration insert for thread {} because it was created concurrently",
                    thread_uuid
                );
                sess.active_thread = Some(thread_uuid);
                sess.last_active_at = chrono::Utc::now();
            } else {
                sess.threads.insert(thread_uuid, thread);
                sess.active_thread = Some(thread_uuid);
                sess.last_active_at = chrono::Utc::now();
            }
        }

        self.session_manager
            .register_thread(
                &message.user_id,
                &message.channel,
                thread_uuid,
                Arc::clone(&session),
            )
            .await;

        tracing::debug!(
            "Hydrated thread {} from DB ({} messages)",
            thread_uuid,
            msg_count
        );
    }

    pub(super) async fn process_user_input(
        &self,
        message: &IncomingMessage,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
        content: &str,
    ) -> Result<SubmissionResult, Error> {
        struct AutoCompactionPlan {
            thread: Thread,
            strategy: CompactionStrategy,
            workspace: Option<Arc<Workspace>>,
            snapshot_updated_at: chrono::DateTime<chrono::Utc>,
            pct: f64,
        }

        // First check thread state without holding lock during I/O
        let thread_state = {
            let mut sess = session.lock().await;
            let thread = sess
                .threads
                .get(&thread_id)
                .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;
            if thread.state == ThreadState::AwaitingApproval
                && is_approval_expired(thread.updated_at)
            {
                // Re-borrow mutably to clear pending approval and return to idle.
                let thread = sess.threads.get_mut(&thread_id).expect("checked above");
                thread.clear_pending_approval();
                if let Some(turn) = thread.last_turn_mut() {
                    turn.record_tool_error("approval timeout");
                }
                return Ok(SubmissionResult::error(
                    "Approval request expired. The pending tool execution was canceled.",
                ));
            }
            thread.state
        };

        // Check thread state
        match thread_state {
            ThreadState::Processing => {
                return Ok(SubmissionResult::error(
                    "Turn in progress. Use /interrupt to cancel.",
                ));
            }
            ThreadState::AwaitingApproval => {
                return Ok(SubmissionResult::error(
                    "Waiting for approval. Use /interrupt to cancel.",
                ));
            }
            ThreadState::Completed => {
                return Ok(SubmissionResult::error(
                    "Thread completed. Use /thread new.",
                ));
            }
            ThreadState::Idle | ThreadState::Interrupted => {
                // Can proceed
            }
        }

        if let Some(shadow) = self.shadow_engine()
            && let Some(cached) = shadow.try_get(content).await
        {
            tracing::info!("Shadow cache hit");
            {
                let mut sess = session.lock().await;
                if let Some(thread) = sess.threads.get_mut(&thread_id) {
                    if let Err(e) = thread.try_start_turn(content) {
                        tracing::warn!(thread_id = %thread_id, "Shadow cache start_turn rejected: {}", e);
                        return Ok(SubmissionResult::error(e));
                    }
                    thread.complete_turn(&cached);
                    self.persist_response_chain(thread);
                }
            }
            let _ = self
                .channels
                .send_status(
                    &message.channel,
                    StatusUpdate::Status("Done (shadow cache)".into()),
                    &message.metadata,
                )
                .await;
            self.persist_turn(
                thread_id,
                &message.user_id,
                content,
                Some(&cached),
                &message.channel,
                &message.metadata,
            );
            return Ok(SubmissionResult::response(cached));
        }

        // Safety validation for user input
        let validation = self.safety().validate_input(content);
        if !validation.is_valid {
            let details = validation
                .errors
                .iter()
                .map(|e| format!("{}: {}", e.field, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Ok(SubmissionResult::error(format!(
                "Input rejected by safety validation: {}",
                details
            )));
        }

        let violations = self.safety().check_policy(content);
        if violations
            .iter()
            .any(|rule| rule.action == crate::safety::PolicyAction::Block)
        {
            return Ok(SubmissionResult::error("Input rejected by safety policy."));
        }

        // Handle explicit commands (starting with /) directly
        // Everything else goes through the normal agentic loop with tools
        let temp_message = IncomingMessage {
            content: content.to_string(),
            ..message.clone()
        };

        if let Some(intent) = self.router.route_command(&temp_message) {
            // Explicit command like /status, /job, /list - handle directly
            return self.handle_job_or_command(intent, message).await;
        }

        if let Some(intent) = self.router.route_fast_path_nl(&temp_message) {
            tracing::debug!("Routed via natural-language fast path");
            return self.handle_job_or_command(intent, message).await;
        }

        if let Some(store) = self.store() {
            let normalized = normalize_reflex_pattern(content);
            match store.find_reflex_tool_for_pattern(&normalized).await {
                Ok(Some(tool_name)) => {
                    if self.tools().has(&tool_name).await {
                        tracing::info!(
                            tool = %tool_name,
                            pattern = %normalized,
                            "Routed via reflex pattern registry"
                        );

                        let job_ctx = JobContext::with_user(
                            &message.user_id,
                            "chat",
                            "Reflex fast-path execution",
                        );
                        let params = serde_json::json!({ "input": content });
                        let mut reflex_allowed = true;
                        if let Some(tool) = self.tools().get(&tool_name).await {
                            let is_auto_approved = {
                                let sess = session.lock().await;
                                sess.is_tool_auto_approved(&tool_name)
                            };
                            if let ApprovalPolicyOutcome::RequireApproval { .. } =
                                evaluate_dispatcher_tool_approval(
                                    tool.as_ref(),
                                    &params,
                                    is_auto_approved,
                                )
                            {
                                tracing::info!(
                                    tool = %tool_name,
                                    "Reflex fast-path requires approval, falling back to agentic loop"
                                );
                                reflex_allowed = false;
                            }
                        }

                        if reflex_allowed {
                            let _ = self
                                .channels
                                .send_status(
                                    &message.channel,
                                    StatusUpdate::Thinking(format!(
                                        "Reflex fast-path: executing {}",
                                        tool_name
                                    )),
                                    &message.metadata,
                                )
                                .await;

                            let live_stream = tool_name == "shell";
                            match self
                                .execute_chat_tool(
                                    &tool_name,
                                    &params,
                                    &job_ctx,
                                    &message.channel,
                                    &message.metadata,
                                    live_stream,
                                )
                                .await
                            {
                                Ok(output) => {
                                    let _ = store.bump_reflex_pattern_hit(&normalized).await;
                                    {
                                        let mut sess = session.lock().await;
                                        if let Some(thread) = sess.threads.get_mut(&thread_id) {
                                            if let Err(e) = thread.try_start_turn(content) {
                                                tracing::warn!(thread_id = %thread_id, "Reflex start_turn rejected: {}", e);
                                                return Ok(SubmissionResult::error(e));
                                            }
                                            thread.complete_turn(&output);
                                            self.persist_response_chain(thread);
                                        }
                                    }
                                    let _ = self
                                        .channels
                                        .send_status(
                                            &message.channel,
                                            StatusUpdate::Status("Done".into()),
                                            &message.metadata,
                                        )
                                        .await;
                                    self.persist_turn(
                                        thread_id,
                                        &message.user_id,
                                        content,
                                        Some(&output),
                                        &message.channel,
                                        &message.metadata,
                                    );
                                    if let Some(shadow) = self.shadow_engine() {
                                        shadow.spawn_speculation(content, &output);
                                    }
                                    return Ok(SubmissionResult::response(output));
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        tool = %tool_name,
                                        "Reflex fast-path failed, falling back to agentic loop: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("Reflex pattern lookup failed: {}", e);
                }
            }
        }

        // Natural language goes through the agentic loop
        // Job tools (create_job, list_jobs, etc.) are in the tool registry

        // Auto-compact if needed BEFORE adding new turn
        let compaction_plan = {
            let sess = session.lock().await;
            if let Some(thread) = sess.threads.get(&thread_id) {
                let messages = thread.messages();
                if let Some(strategy) = self.context_monitor.suggest_compaction(&messages) {
                    let pct = self.context_monitor.usage_percent(&messages);
                    tracing::info!("Context at {:.1}% capacity, auto-compacting", pct);

                    Some(AutoCompactionPlan {
                        thread: thread.clone(),
                        strategy,
                        workspace: self.workspace().map(Arc::clone),
                        snapshot_updated_at: thread.updated_at,
                        pct,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(plan) = compaction_plan {
            let AutoCompactionPlan {
                thread: mut compacted_thread,
                strategy,
                workspace,
                snapshot_updated_at,
                pct,
            } = plan;

            // Notify the user that compaction is happening
            let _ = self
                .channels
                .send_status(
                    &message.channel,
                    StatusUpdate::Status(format!("Context at {:.0}% capacity, compacting...", pct)),
                    &message.metadata,
                )
                .await;

            let compactor = ContextCompactor::new(self.llm().clone());
            if let Err(e) = compactor
                .compact(&mut compacted_thread, strategy, workspace.as_deref())
                .await
            {
                tracing::warn!("Auto-compaction failed: {}", e);
            } else {
                let mut sess = session.lock().await;
                if let Some(thread) = sess.threads.get_mut(&thread_id)
                    && thread.updated_at == snapshot_updated_at
                {
                    *thread = compacted_thread;
                    sess.last_active_at = chrono::Utc::now();
                } else {
                    tracing::debug!("Skipped auto-compaction because thread changed");
                }
            }
        }

        // Create checkpoint before turn
        let undo_mgr = self.session_manager.get_undo_manager(thread_id).await;
        {
            let sess = session.lock().await;
            let thread = sess
                .threads
                .get(&thread_id)
                .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

            let mut mgr = undo_mgr.lock().await;
            mgr.checkpoint(
                thread.turn_number(),
                thread.messages(),
                format!("Before turn {}", thread.turn_number()),
            );
        }

        // Start the turn and get messages
        let turn_messages = {
            let mut sess = session.lock().await;
            let thread = sess
                .threads
                .get_mut(&thread_id)
                .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;
            thread.try_start_turn(content).map_err(|e| {
                Error::from(crate::error::JobError::ContextError {
                    id: thread_id,
                    reason: e,
                })
            })?;
            thread.messages()
        };

        // Send thinking status
        let _ = self
            .channels
            .send_status(
                &message.channel,
                StatusUpdate::Thinking("Processing...".into()),
                &message.metadata,
            )
            .await;

        // Run the agentic tool execution loop
        let result = self
            .run_agentic_loop(message, session.clone(), thread_id, turn_messages, false)
            .await;

        // Re-acquire lock and check if interrupted
        let mut sess = session.lock().await;
        let thread = sess
            .threads
            .get_mut(&thread_id)
            .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

        if thread.state == ThreadState::Interrupted {
            let _ = self
                .channels
                .send_status(
                    &message.channel,
                    StatusUpdate::Status("Interrupted".into()),
                    &message.metadata,
                )
                .await;
            return Ok(SubmissionResult::Interrupted);
        }

        // Complete, fail, or request approval
        match result {
            Ok(AgenticLoopResult::Response(response)) => {
                // Hook: TransformResponse — allow hooks to modify or reject the final response
                let response = {
                    let event = crate::hooks::HookEvent::ResponseTransform {
                        user_id: message.user_id.clone(),
                        thread_id: thread_id.to_string(),
                        response: response.clone(),
                    };
                    match self.hooks().run(&event).await {
                        Err(crate::hooks::HookError::Rejected { reason }) => {
                            format!("[Response filtered: {}]", reason)
                        }
                        Err(err) => {
                            format!("[Response blocked by hook policy: {}]", err)
                        }
                        Ok(crate::hooks::HookOutcome::Continue {
                            modified: Some(new_response),
                        }) => new_response,
                        _ => response, // fail-open: use original
                    }
                };

                thread.complete_turn(&response);
                self.persist_response_chain(thread);
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::Status("Done".into()),
                        &message.metadata,
                    )
                    .await;

                // Fire-and-forget: persist turn to DB
                self.persist_turn(
                    thread_id,
                    &message.user_id,
                    content,
                    Some(&response),
                    &message.channel,
                    &message.metadata,
                );
                if let Some(shadow) = self.shadow_engine() {
                    shadow.spawn_speculation(content, &response);
                }

                Ok(SubmissionResult::response(response))
            }
            Ok(AgenticLoopResult::NeedApproval { pending }) => {
                // Store pending approval in thread and update state
                let request_id = pending.request_id;
                let tool_name = pending.tool_name.clone();
                let description = pending.description.clone();
                let parameters = pending.parameters.clone();
                thread.await_approval(pending);
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::Status("Awaiting approval".into()),
                        &message.metadata,
                    )
                    .await;
                Ok(SubmissionResult::NeedApproval {
                    request_id,
                    tool_name,
                    description,
                    parameters,
                })
            }
            Err(e) => {
                thread.fail_turn(e.to_string());

                // Persist the user message even on failure
                self.persist_turn(
                    thread_id,
                    &message.user_id,
                    content,
                    None,
                    &message.channel,
                    &message.metadata,
                );

                Ok(SubmissionResult::error(e.to_string()))
            }
        }
    }

    /// Fire-and-forget: persist a turn (user message + optional assistant response) to the DB.
    pub(super) fn persist_turn(
        &self,
        thread_id: Uuid,
        user_id: &str,
        user_input: &str,
        response: Option<&str>,
        channel: &str,
        metadata: &serde_json::Value,
    ) {
        if let Some(resp) = response
            && let Some(synth) = self.profile_synthesizer()
        {
            synth.enqueue_turn(user_id, &thread_id.to_string(), user_input, resp);
        }

        let store = match self.store() {
            Some(s) => Arc::clone(s),
            None => return,
        };

        let user_id = user_id.to_string();
        let user_input = user_input.to_string();
        let response = response.map(String::from);
        let channels = self.channels.clone();
        let channel = channel.to_string();
        let metadata = metadata.clone();

        tokio::spawn(async move {
            let mut last_err: Option<String> = None;
            for attempt in 1..=3 {
                let result = async {
                    store
                        .ensure_conversation(thread_id, "gateway", &user_id, None)
                        .await?;
                    store
                        .add_conversation_message(thread_id, "user", &user_input)
                        .await?;
                    if let Some(ref resp) = response {
                        store
                            .add_conversation_message(thread_id, "assistant", resp)
                            .await?;
                    }
                    Ok::<(), crate::error::DatabaseError>(())
                }
                .await;

                match result {
                    Ok(()) => return,
                    Err(e) => {
                        last_err = Some(e.to_string());
                        tracing::warn!(
                            thread_id = %thread_id,
                            attempt,
                            "Failed to persist turn: {}",
                            e
                        );
                        if attempt < 3 {
                            tokio::time::sleep(std::time::Duration::from_millis(150 * attempt))
                                .await;
                        }
                    }
                }
            }

            let warn_msg = format!(
                "Warning: response completed, but chat history could not be saved after retries{}",
                last_err
                    .as_deref()
                    .map(|e| format!(" ({})", e))
                    .unwrap_or_default()
            );
            let _ = channels
                .send_status(&channel, StatusUpdate::Status(warn_msg), &metadata)
                .await;
        });
    }

    /// Sync the provider's response chain ID to the thread and DB metadata.
    ///
    /// Call after a successful agentic loop to persist the latest
    /// `previous_response_id` so chaining survives restarts.
    pub(super) fn persist_response_chain(&self, thread: &mut crate::agent::session::Thread) {
        let tid = thread.id.to_string();
        let response_id = match self.llm().get_response_chain_id(&tid) {
            Some(rid) => rid,
            None => return,
        };

        // Update in-memory thread
        thread.last_response_id = Some(response_id.clone());

        // Fire-and-forget DB write
        let store = match self.store() {
            Some(s) => Arc::clone(s),
            None => return,
        };
        let thread_id = thread.id;
        tokio::spawn(async move {
            let val = serde_json::json!(response_id);
            if let Err(e) = store
                .update_conversation_metadata_field(thread_id, "last_response_id", &val)
                .await
            {
                tracing::warn!(
                    "Failed to persist response chain for thread {}: {}",
                    thread_id,
                    e
                );
            }
        });
    }

    pub(super) async fn process_undo(
        &self,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
    ) -> Result<SubmissionResult, Error> {
        let undo_mgr = self.session_manager.get_undo_manager(thread_id).await;
        let mut mgr = undo_mgr.lock().await;

        if !mgr.can_undo() {
            return Ok(SubmissionResult::ok_with_message("Nothing to undo."));
        }

        let mut sess = session.lock().await;
        let thread = sess
            .threads
            .get_mut(&thread_id)
            .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

        // Save current state to redo, get previous checkpoint
        let current_messages = thread.messages();
        let current_turn = thread.turn_number();

        if let Some(checkpoint) = mgr.undo(current_turn, current_messages) {
            // Extract values before consuming the reference
            let turn_number = checkpoint.turn_number;
            let messages = checkpoint.messages.clone();
            let undo_count = mgr.undo_count();
            // Restore thread from checkpoint
            thread.restore_from_messages(messages);
            Ok(SubmissionResult::ok_with_message(format!(
                "Undone to turn {}. {} undo(s) remaining.",
                turn_number, undo_count
            )))
        } else {
            Ok(SubmissionResult::error("Undo failed."))
        }
    }

    pub(super) async fn process_redo(
        &self,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
    ) -> Result<SubmissionResult, Error> {
        let undo_mgr = self.session_manager.get_undo_manager(thread_id).await;
        let mut mgr = undo_mgr.lock().await;

        if !mgr.can_redo() {
            return Ok(SubmissionResult::ok_with_message("Nothing to redo."));
        }

        let mut sess = session.lock().await;
        let thread = sess
            .threads
            .get_mut(&thread_id)
            .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

        let current_messages = thread.messages();
        let current_turn = thread.turn_number();

        if let Some(checkpoint) = mgr.redo(current_turn, current_messages) {
            thread.restore_from_messages(checkpoint.messages);
            Ok(SubmissionResult::ok_with_message(format!(
                "Redone to turn {}.",
                checkpoint.turn_number
            )))
        } else {
            Ok(SubmissionResult::error("Redo failed."))
        }
    }

    pub(super) async fn process_interrupt(
        &self,
        message: &IncomingMessage,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
    ) -> Result<SubmissionResult, Error> {
        let mut sess = session.lock().await;
        let thread = sess
            .threads
            .get_mut(&thread_id)
            .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

        match thread.state {
            ThreadState::Processing | ThreadState::AwaitingApproval => {
                thread.interrupt();
                drop(sess);

                let mut canceled = 0usize;
                let active_jobs = self.context_manager.active_jobs_for(&message.user_id).await;
                for job_id in active_jobs {
                    if self.scheduler.stop(job_id).await.is_ok() {
                        canceled += 1;
                    }
                    if let Some(jm) = &self.deps.container_job_manager
                        && jm.stop_job(job_id).await.is_ok()
                    {
                        canceled += 1;
                    }
                }

                if canceled > 0 {
                    Ok(SubmissionResult::ok_with_message(format!(
                        "Interrupted. Requested cancellation for {} active job(s).",
                        canceled
                    )))
                } else {
                    Ok(SubmissionResult::ok_with_message("Interrupted."))
                }
            }
            _ => Ok(SubmissionResult::ok_with_message("Nothing to interrupt.")),
        }
    }

    pub(super) async fn process_compact(
        &self,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
    ) -> Result<SubmissionResult, Error> {
        let (mut compacted_thread, snapshot_updated_at, usage, strategy) = {
            let sess = session.lock().await;
            let thread = sess
                .threads
                .get(&thread_id)
                .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

            let messages = thread.messages();
            let usage = self.context_monitor.usage_percent(&messages);
            let strategy = self
                .context_monitor
                .suggest_compaction(&messages)
                .unwrap_or(
                    crate::agent::context_monitor::CompactionStrategy::Summarize { keep_recent: 5 },
                );
            (thread.clone(), thread.updated_at, usage, strategy)
        };

        let compactor = ContextCompactor::new(self.llm().clone());
        match compactor
            .compact(
                &mut compacted_thread,
                strategy,
                self.workspace().map(|w| w.as_ref()),
            )
            .await
        {
            Ok(result) => {
                let mut sess = session.lock().await;
                if let Some(thread) = sess.threads.get_mut(&thread_id)
                    && thread.updated_at == snapshot_updated_at
                {
                    *thread = compacted_thread;
                    sess.last_active_at = chrono::Utc::now();
                } else {
                    return Ok(SubmissionResult::ok_with_message(
                        "Compaction skipped because the thread changed during compaction.",
                    ));
                }

                let mut msg = format!(
                    "Compacted: {} turns removed, {} → {} tokens (was {:.1}% full)",
                    result.turns_removed, result.tokens_before, result.tokens_after, usage
                );
                if result.summary_written {
                    msg.push_str(", summary saved to workspace");
                }
                Ok(SubmissionResult::ok_with_message(msg))
            }
            Err(e) => Ok(SubmissionResult::error(format!("Compaction failed: {}", e))),
        }
    }

    pub(super) async fn process_clear(
        &self,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
    ) -> Result<SubmissionResult, Error> {
        let mut sess = session.lock().await;
        let thread = sess
            .threads
            .get_mut(&thread_id)
            .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;
        thread.turns.clear();
        thread.state = ThreadState::Idle;

        // Clear undo history too
        let undo_mgr = self.session_manager.get_undo_manager(thread_id).await;
        undo_mgr.lock().await.clear();

        Ok(SubmissionResult::ok_with_message("Thread cleared."))
    }

    /// Process an approval or rejection of a pending tool execution.
    pub(super) async fn process_approval(
        &self,
        message: &IncomingMessage,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
        request_id: Option<Uuid>,
        approved: bool,
        always: bool,
    ) -> Result<SubmissionResult, Error> {
        // Get thread state and pending approval
        let (_thread_state, pending) = {
            let mut sess = session.lock().await;
            let thread = sess
                .threads
                .get_mut(&thread_id)
                .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

            if thread.state != ThreadState::AwaitingApproval {
                return Ok(SubmissionResult::error("No pending approval request."));
            }
            if is_approval_expired(thread.updated_at) {
                thread.clear_pending_approval();
                if let Some(turn) = thread.last_turn_mut() {
                    turn.record_tool_error("approval timeout");
                }
                return Ok(SubmissionResult::error(
                    "Approval request expired. Ask again to retry the operation.",
                ));
            }

            let pending = thread.take_pending_approval();
            (thread.state, pending)
        };

        let pending = match pending {
            Some(p) => p,
            None => return Ok(SubmissionResult::error("No pending approval request.")),
        };

        // Verify request ID if provided
        if let Some(req_id) = request_id
            && req_id != pending.request_id
        {
            // Put it back and return error
            let mut sess = session.lock().await;
            if let Some(thread) = sess.threads.get_mut(&thread_id) {
                thread.await_approval(pending);
            }
            return Ok(SubmissionResult::error(
                "Request ID mismatch. Use the correct request ID.",
            ));
        }

        if approved {
            // If always, add to auto-approved set
            if always {
                let mut sess = session.lock().await;
                sess.auto_approve_tool(&pending.tool_name);
                tracing::info!(
                    "Auto-approved tool '{}' for session {}",
                    pending.tool_name,
                    sess.id
                );
            }

            // Reset thread state to processing
            {
                let mut sess = session.lock().await;
                if let Some(thread) = sess.threads.get_mut(&thread_id) {
                    thread.state = ThreadState::Processing;
                }
            }

            // Execute the approved tool and continue the loop
            let job_ctx =
                JobContext::with_user(&message.user_id, "chat", "Interactive chat session");
            let mut approved_params = pending.parameters.clone();
            if self.config.autonomy_policy_engine_v1 {
                persist_chat_policy_decision_best_effort(
                    self,
                    message,
                    thread_id,
                    &job_ctx,
                    &pending.tool_name,
                    &pending.tool_call_id,
                    TelemetryPolicyDecisionKind::Allow,
                    vec![if always {
                        "user_approved_tool_and_enabled_auto_approval".to_string()
                    } else {
                        "user_approved_tool".to_string()
                    }],
                    Some(always),
                    "approval_resume_decision",
                );

                match evaluate_tool_call_hook(
                    self.hooks().as_ref(),
                    &pending.tool_name,
                    &approved_params,
                    &message.user_id,
                    "chat_approval_resume",
                )
                .await
                {
                    HookToolPolicyOutcome::Continue { parameters } => {
                        approved_params = parameters;
                    }
                    HookToolPolicyOutcome::Modified {
                        parameters,
                        reason_codes,
                    } => {
                        persist_chat_policy_decision_best_effort(
                            self,
                            message,
                            thread_id,
                            &job_ctx,
                            &pending.tool_name,
                            &pending.tool_call_id,
                            TelemetryPolicyDecisionKind::Modify,
                            reason_codes,
                            None,
                            "approval_resume_hook",
                        );
                        approved_params = parameters;
                    }
                    HookToolPolicyOutcome::Deny {
                        reason,
                        reason_codes,
                    } => {
                        persist_chat_policy_decision_best_effort(
                            self,
                            message,
                            thread_id,
                            &job_ctx,
                            &pending.tool_name,
                            &pending.tool_call_id,
                            TelemetryPolicyDecisionKind::Deny,
                            reason_codes,
                            None,
                            "approval_resume_hook",
                        );
                        let blocked_msg = format!(
                            "Tool '{}' was approved, but a policy hook blocked execution: {}",
                            pending.tool_name, reason
                        );
                        let persist_user_input = {
                            let mut sess = session.lock().await;
                            if let Some(thread) = sess.threads.get_mut(&thread_id) {
                                if let Some(turn) = thread.last_turn_mut() {
                                    turn.record_tool_error(format!(
                                        "blocked by hook after approval: {}",
                                        reason
                                    ));
                                }
                                let user_input = thread.last_turn().map(|t| t.user_input.clone());
                                thread.complete_turn(&blocked_msg);
                                user_input
                            } else {
                                None
                            }
                        };
                        let _ = self
                            .channels
                            .send_status(
                                &message.channel,
                                StatusUpdate::Status("Blocked by policy".into()),
                                &message.metadata,
                            )
                            .await;
                        if let Some(user_input) = persist_user_input {
                            self.persist_turn(
                                thread_id,
                                &message.user_id,
                                &user_input,
                                Some(&blocked_msg),
                                &message.channel,
                                &message.metadata,
                            );
                        }
                        return Ok(SubmissionResult::response(blocked_msg));
                    }
                }
            }

            let _ = self
                .channels
                .send_status(
                    &message.channel,
                    StatusUpdate::ToolStarted {
                        name: pending.tool_name.clone(),
                    },
                    &message.metadata,
                )
                .await;

            let tool_result = self
                .execute_chat_tool(
                    &pending.tool_name,
                    &approved_params,
                    &job_ctx,
                    &message.channel,
                    &message.metadata,
                    pending.tool_name == "shell",
                )
                .await;

            let _ = self
                .channels
                .send_status(
                    &message.channel,
                    StatusUpdate::ToolCompleted {
                        name: pending.tool_name.clone(),
                        success: tool_result.is_ok(),
                    },
                    &message.metadata,
                )
                .await;

            if pending.tool_name != "shell"
                && let Ok(ref output) = tool_result
                && !output.is_empty()
            {
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::ToolResult {
                            name: pending.tool_name.clone(),
                            preview: output.clone(),
                        },
                        &message.metadata,
                    )
                    .await;
            }

            // Build context including the tool result
            let mut context_messages = pending.context_messages;

            let result_content = match &tool_result {
                Ok(output) => {
                    let sanitized = self
                        .safety()
                        .sanitize_tool_output(&pending.tool_name, &output);
                    self.safety().wrap_for_llm(
                        &pending.tool_name,
                        &sanitized.content,
                        sanitized.was_modified,
                    )
                }
                Err(e) => format!("Error: {}", e),
            };

            // Record result in thread
            {
                let mut sess = session.lock().await;
                if let Some(thread) = sess.threads.get_mut(&thread_id)
                    && let Some(turn) = thread.last_turn_mut()
                {
                    match &tool_result {
                        Ok(output) => {
                            turn.record_tool_result(
                                serde_json::json!(output),
                                Some(result_content.clone()),
                            );
                        }
                        Err(e) => {
                            turn.record_tool_error(e.to_string());
                        }
                    }
                }
            }

            // If tool_auth returned awaiting_token, enter auth mode and
            // return instructions directly (skip agentic loop continuation).
            if let Some((ext_name, instructions)) =
                detect_auth_awaiting(&pending.tool_name, &tool_result)
            {
                let auth_data = parse_auth_result(&tool_result);
                {
                    let mut sess = session.lock().await;
                    if let Some(thread) = sess.threads.get_mut(&thread_id) {
                        thread.enter_auth_mode(ext_name.clone());
                        thread.complete_turn(&instructions);
                    }
                }
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::AuthRequired {
                            extension_name: ext_name,
                            instructions: Some(instructions.clone()),
                            auth_url: auth_data.auth_url,
                            setup_url: auth_data.setup_url,
                        },
                        &message.metadata,
                    )
                    .await;
                return Ok(SubmissionResult::response(instructions));
            }

            // Add tool result to context
            context_messages.push(ChatMessage::tool_result(
                &pending.tool_call_id,
                &pending.tool_name,
                result_content,
            ));

            // Continue the agentic loop (a tool was already executed this turn)
            let result = self
                .run_agentic_loop(message, session.clone(), thread_id, context_messages, true)
                .await;

            // Handle the result
            let mut sess = session.lock().await;
            let thread = sess
                .threads
                .get_mut(&thread_id)
                .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;

            match result {
                Ok(AgenticLoopResult::Response(response)) => {
                    thread.complete_turn(&response);
                    self.persist_response_chain(thread);
                    let persist_user_input = thread.last_turn().map(|t| t.user_input.clone());
                    let _ = self
                        .channels
                        .send_status(
                            &message.channel,
                            StatusUpdate::Status("Done".into()),
                            &message.metadata,
                        )
                        .await;
                    if let Some(user_input) = persist_user_input {
                        self.persist_turn(
                            thread_id,
                            &message.user_id,
                            &user_input,
                            Some(&response),
                            &message.channel,
                            &message.metadata,
                        );
                    }
                    Ok(SubmissionResult::response(response))
                }
                Ok(AgenticLoopResult::NeedApproval {
                    pending: new_pending,
                }) => {
                    let request_id = new_pending.request_id;
                    let tool_name = new_pending.tool_name.clone();
                    let description = new_pending.description.clone();
                    let parameters = new_pending.parameters.clone();
                    thread.await_approval(new_pending);
                    let _ = self
                        .channels
                        .send_status(
                            &message.channel,
                            StatusUpdate::Status("Awaiting approval".into()),
                            &message.metadata,
                        )
                        .await;
                    Ok(SubmissionResult::NeedApproval {
                        request_id,
                        tool_name,
                        description,
                        parameters,
                    })
                }
                Err(e) => {
                    thread.fail_turn(e.to_string());
                    let persist_user_input = thread.last_turn().map(|t| t.user_input.clone());
                    if let Some(user_input) = persist_user_input {
                        self.persist_turn(
                            thread_id,
                            &message.user_id,
                            &user_input,
                            None,
                            &message.channel,
                            &message.metadata,
                        );
                    }
                    Ok(SubmissionResult::error(e.to_string()))
                }
            }
        } else {
            // Rejected - clear approval and return to idle
            if self.config.autonomy_policy_engine_v1 {
                let job_ctx =
                    JobContext::with_user(&message.user_id, "chat", "Interactive chat session");
                persist_chat_policy_decision_best_effort(
                    self,
                    message,
                    thread_id,
                    &job_ctx,
                    &pending.tool_name,
                    &pending.tool_call_id,
                    TelemetryPolicyDecisionKind::Deny,
                    vec!["user_rejected_tool".to_string()],
                    Some(false),
                    "approval_resume_decision",
                );
            }
            let rejection_msg = format!(
                "Tool '{}' was rejected. The agent will not execute this tool.\n\n\
                 You can continue the conversation or try a different approach.",
                pending.tool_name
            );
            let persist_user_input = {
                let mut sess = session.lock().await;
                if let Some(thread) = sess.threads.get_mut(&thread_id) {
                    if let Some(turn) = thread.last_turn_mut() {
                        turn.record_tool_error("rejected by user");
                    }
                    let user_input = thread.last_turn().map(|t| t.user_input.clone());
                    thread.clear_pending_approval();
                    thread.complete_turn(&rejection_msg);
                    user_input
                } else {
                    None
                }
            };

            let _ = self
                .channels
                .send_status(
                    &message.channel,
                    StatusUpdate::Status("Rejected".into()),
                    &message.metadata,
                )
                .await;
            if let Some(user_input) = persist_user_input {
                self.persist_turn(
                    thread_id,
                    &message.user_id,
                    &user_input,
                    Some(&rejection_msg),
                    &message.channel,
                    &message.metadata,
                );
            }

            Ok(SubmissionResult::response(rejection_msg))
        }
    }

    /// Handle an auth token submitted while the thread is in auth mode.
    ///
    /// The token goes directly to the extension manager's credential store,
    /// completely bypassing logging, turn creation, history, and compaction.
    pub(super) async fn process_auth_token(
        &self,
        message: &IncomingMessage,
        pending: &crate::agent::session::PendingAuth,
        token: &str,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
    ) -> Result<Option<String>, Error> {
        let token = token.trim();

        // Clear auth mode regardless of outcome
        {
            let mut sess = session.lock().await;
            if let Some(thread) = sess.threads.get_mut(&thread_id) {
                thread.pending_auth = None;
            }
        }

        let ext_mgr = match self.deps.extension_manager.as_ref() {
            Some(mgr) => mgr,
            None => return Ok(Some("Extension manager not available.".to_string())),
        };

        match ext_mgr.auth(&pending.extension_name, Some(token)).await {
            Ok(result) if result.status == "authenticated" => {
                tracing::info!(
                    "Extension '{}' authenticated via auth mode",
                    pending.extension_name
                );

                // Auto-activate so tools are available immediately after auth
                match ext_mgr.activate(&pending.extension_name).await {
                    Ok(activate_result) => {
                        let tool_count = activate_result.tools_loaded.len();
                        let tool_list = if activate_result.tools_loaded.is_empty() {
                            String::new()
                        } else {
                            format!("\n\nTools: {}", activate_result.tools_loaded.join(", "))
                        };
                        let msg = format!(
                            "{} authenticated and activated ({} tools loaded).{}",
                            pending.extension_name, tool_count, tool_list
                        );
                        let _ = self
                            .channels
                            .send_status(
                                &message.channel,
                                StatusUpdate::AuthCompleted {
                                    extension_name: pending.extension_name.clone(),
                                    success: true,
                                    message: msg.clone(),
                                },
                                &message.metadata,
                            )
                            .await;
                        Ok(Some(msg))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Extension '{}' authenticated but activation failed: {}",
                            pending.extension_name,
                            e
                        );
                        let msg = format!(
                            "{} authenticated successfully, but activation failed: {}. \
                             Try activating manually.",
                            pending.extension_name, e
                        );
                        let _ = self
                            .channels
                            .send_status(
                                &message.channel,
                                StatusUpdate::AuthCompleted {
                                    extension_name: pending.extension_name.clone(),
                                    success: true,
                                    message: msg.clone(),
                                },
                                &message.metadata,
                            )
                            .await;
                        Ok(Some(msg))
                    }
                }
            }
            Ok(result) => {
                // Invalid token, re-enter auth mode
                {
                    let mut sess = session.lock().await;
                    if let Some(thread) = sess.threads.get_mut(&thread_id) {
                        thread.enter_auth_mode(pending.extension_name.clone());
                    }
                }
                let msg = result
                    .instructions
                    .clone()
                    .unwrap_or_else(|| "Invalid token. Please try again.".to_string());
                // Re-emit AuthRequired so web UI re-shows the card
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::AuthRequired {
                            extension_name: pending.extension_name.clone(),
                            instructions: Some(msg.clone()),
                            auth_url: result.auth_url,
                            setup_url: result.setup_url,
                        },
                        &message.metadata,
                    )
                    .await;
                Ok(Some(msg))
            }
            Err(e) => {
                let msg = format!(
                    "Authentication failed for {}: {}",
                    pending.extension_name, e
                );
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::AuthCompleted {
                            extension_name: pending.extension_name.clone(),
                            success: false,
                            message: msg.clone(),
                        },
                        &message.metadata,
                    )
                    .await;
                Ok(Some(msg))
            }
        }
    }

    pub(super) async fn process_new_thread(
        &self,
        message: &IncomingMessage,
    ) -> Result<SubmissionResult, Error> {
        let session = self
            .session_manager
            .get_or_create_session(&message.user_id)
            .await;
        let mut sess = session.lock().await;
        let thread = sess.create_thread();
        let thread_id = thread.id;
        Ok(SubmissionResult::ok_with_message(format!(
            "New thread: {}",
            thread_id
        )))
    }

    pub(super) async fn process_switch_thread(
        &self,
        message: &IncomingMessage,
        target_thread_id: Uuid,
    ) -> Result<SubmissionResult, Error> {
        let session = self
            .session_manager
            .get_or_create_session(&message.user_id)
            .await;
        let mut sess = session.lock().await;

        if sess.switch_thread(target_thread_id) {
            Ok(SubmissionResult::ok_with_message(format!(
                "Switched to thread {}",
                target_thread_id
            )))
        } else {
            Ok(SubmissionResult::error("Thread not found."))
        }
    }

    pub(super) async fn process_resume(
        &self,
        session: Arc<Mutex<Session>>,
        thread_id: Uuid,
        checkpoint_id: Uuid,
    ) -> Result<SubmissionResult, Error> {
        let undo_mgr = self.session_manager.get_undo_manager(thread_id).await;
        let mut mgr = undo_mgr.lock().await;

        if let Some(checkpoint) = mgr.restore(checkpoint_id) {
            let mut sess = session.lock().await;
            let thread = sess
                .threads
                .get_mut(&thread_id)
                .ok_or_else(|| Error::from(crate::error::JobError::NotFound { id: thread_id }))?;
            thread.restore_from_messages(checkpoint.messages);
            Ok(SubmissionResult::ok_with_message(format!(
                "Resumed from checkpoint: {}",
                checkpoint.description
            )))
        } else {
            Ok(SubmissionResult::error("Checkpoint not found."))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::channels::{ChannelManager, IncomingMessage};
    use crate::config::AgentConfig;
    use crate::hooks::{Hook, HookContext, HookEvent, HookFailureMode, HookOutcome, HookPoint};
    use crate::llm::ChatMessage;
    use crate::settings::Settings;
    use crate::testing::TestHarnessBuilder;

    struct RejectToolCallHook;

    #[async_trait]
    impl Hook for RejectToolCallHook {
        fn name(&self) -> &str {
            "reject_tool_call"
        }

        fn hook_points(&self) -> &[HookPoint] {
            static POINTS: [HookPoint; 1] = [HookPoint::BeforeToolCall];
            &POINTS
        }

        fn failure_mode(&self) -> HookFailureMode {
            HookFailureMode::FailClosed
        }

        fn timeout(&self) -> Duration {
            Duration::from_secs(1)
        }

        async fn execute(
            &self,
            event: &HookEvent,
            _ctx: &HookContext,
        ) -> Result<HookOutcome, crate::hooks::HookError> {
            match event {
                HookEvent::ToolCall { .. } => Ok(HookOutcome::reject("blocked in test hook")),
                _ => Ok(HookOutcome::ok()),
            }
        }
    }

    #[cfg(feature = "libsql")]
    #[tokio::test]
    async fn test_process_approval_rechecks_hook_and_blocks_execution() {
        let harness = TestHarnessBuilder::new().build().await;
        harness
            .deps
            .hooks
            .register(Arc::new(RejectToolCallHook))
            .await;

        let mut config = AgentConfig::resolve(&Settings::default()).expect("config");
        config.autonomy_policy_engine_v1 = true;

        let agent = Agent::new(
            config,
            harness.deps,
            ChannelManager::new(),
            None,
            None,
            None,
            None,
        );

        let message = IncomingMessage::new("test", "user-1", "approve")
            .with_metadata(json!({"source":"test"}));
        let session = agent.session_manager.get_or_create_session("user-1").await;

        let (thread_id, request_id) = {
            let mut sess = session.lock().await;
            let thread = sess.create_thread();
            thread.start_turn("Please run shell");
            thread.record_tool_call("call-1", "shell", json!({"command":"echo hi"}));
            let request_id = Uuid::new_v4();
            thread.await_approval(crate::agent::PendingApproval {
                request_id,
                tool_name: "shell".to_string(),
                parameters: json!({"command":"echo hi"}),
                description: "Run shell".to_string(),
                tool_call_id: "call-1".to_string(),
                context_messages: vec![ChatMessage::user("Please run shell")],
            });
            (thread.id, request_id)
        };

        let out = agent
            .process_approval(
                &message,
                session.clone(),
                thread_id,
                Some(request_id),
                true,
                false,
            )
            .await
            .expect("process approval");

        match out {
            crate::agent::submission::SubmissionResult::Response { content } => {
                assert!(content.contains("policy hook blocked execution"));
            }
            other => panic!("unexpected submission result: {:?}", other),
        }

        let sess = session.lock().await;
        let thread = sess.threads.get(&thread_id).expect("thread");
        assert_eq!(thread.state, ThreadState::Idle);
        assert!(thread.pending_approval.is_none());
        let turn = thread.last_turn().expect("turn");
        assert!(
            turn.response
                .as_deref()
                .unwrap_or_default()
                .contains("policy hook blocked execution")
        );
        assert_eq!(turn.tool_calls.len(), 1);
        assert!(
            turn.tool_calls[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("blocked by hook after approval")
        );
    }
}

fn is_approval_expired(updated_at: chrono::DateTime<chrono::Utc>) -> bool {
    updated_at < chrono::Utc::now() - chrono::TimeDelta::seconds(APPROVAL_TIMEOUT_SECS)
}
