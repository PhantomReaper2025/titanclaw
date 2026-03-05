//! Chat handlers: send, approval, auth, SSE events, WebSocket, history, threads.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use crate::channels::IncomingMessage;
use crate::channels::web::server::GatewayState;
use crate::channels::web::types::*;

pub async fn chat_send_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), (StatusCode, String)> {
    if !state.chat_rate_limiter.check() {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded. Try again shortly.".to_string(),
        ));
    }

    let mut msg = IncomingMessage::new("gateway", &state.user_id, &req.content);

    if let Some(ref thread_id) = req.thread_id {
        msg = msg.with_thread(thread_id);
        msg = msg.with_metadata(serde_json::json!({"thread_id": thread_id}));
    }

    let msg_id = msg.id;

    let tx_guard = state.msg_tx.read().await;
    let tx = tx_guard.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Channel not started".to_string(),
    ))?;

    tx.send(msg).await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Channel closed".to_string(),
        )
    })?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SendMessageResponse {
            message_id: msg_id,
            status: "accepted",
        }),
    ))
}

pub async fn chat_approval_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ApprovalRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), (StatusCode, String)> {
    let (approved, always) = match req.action.as_str() {
        "approve" => (true, false),
        "always" => (true, true),
        "deny" => (false, false),
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown action: {}", other),
            ));
        }
    };

    let request_id = Uuid::parse_str(&req.request_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "Invalid request_id (expected UUID)".to_string(),
        )
    })?;

    // Build a structured ExecApproval submission as JSON, sent through the
    // existing message pipeline so the agent loop picks it up.
    let approval = crate::agent::submission::Submission::ExecApproval {
        request_id,
        approved,
        always,
    };
    let content = serde_json::to_string(&approval).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize approval: {}", e),
        )
    })?;

    let mut msg = IncomingMessage::new("gateway", &state.user_id, content);

    if let Some(ref thread_id) = req.thread_id {
        msg = msg.with_thread(thread_id);
        msg = msg.with_metadata(serde_json::json!({"thread_id": thread_id}));
    }

    let msg_id = msg.id;

    let tx_guard = state.msg_tx.read().await;
    let tx = tx_guard.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Channel not started".to_string(),
    ))?;

    tx.send(msg).await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Channel closed".to_string(),
        )
    })?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SendMessageResponse {
            message_id: msg_id,
            status: "accepted",
        }),
    ))
}

async fn resolve_auth_thread_id(
    state: &GatewayState,
    requested_thread_id: Option<&str>,
) -> Option<String> {
    crate::channels::web::server::resolve_auth_thread_id(state, requested_thread_id).await
}

/// Submit an auth token directly to the extension manager, bypassing the message pipeline.
///
/// The token never touches the LLM, chat history, or SSE stream.
pub async fn chat_auth_token_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AuthTokenRequest>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    let ext_mgr = state.extension_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Extension manager not available".to_string(),
    ))?;

    let result = ext_mgr
        .auth(&req.extension_name, Some(&req.token))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let thread_id = resolve_auth_thread_id(&state, req.thread_id.as_deref()).await;

    if result.status == "authenticated" {
        // Auto-activate so tools are available immediately
        let msg = match ext_mgr.activate(&req.extension_name).await {
            Ok(r) => format!(
                "{} authenticated ({} tools loaded)",
                req.extension_name,
                r.tools_loaded.len()
            ),
            Err(e) => format!(
                "{} authenticated but activation failed: {}",
                req.extension_name, e
            ),
        };

        // Clear auth mode on the active thread
        clear_auth_mode(&state, req.thread_id.as_deref()).await;

        state.sse.broadcast(SseEvent::AuthCompleted {
            extension_name: req.extension_name,
            success: true,
            message: msg.clone(),
            thread_id,
        });

        Ok(Json(ActionResponse::ok(msg)))
    } else {
        // Re-emit auth_required for retry
        state.sse.broadcast(SseEvent::AuthRequired {
            extension_name: req.extension_name.clone(),
            instructions: result.instructions.clone(),
            auth_url: result.auth_url.clone(),
            setup_url: result.setup_url.clone(),
            thread_id,
        });
        Ok(Json(ActionResponse::fail(
            result
                .instructions
                .unwrap_or_else(|| "Invalid token".to_string()),
        )))
    }
}

/// Cancel an in-progress auth flow.
pub async fn chat_auth_cancel_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AuthCancelRequest>,
) -> Result<Json<ActionResponse>, (StatusCode, String)> {
    clear_auth_mode(&state, req.thread_id.as_deref()).await;
    Ok(Json(ActionResponse::ok("Auth cancelled")))
}

/// Clear pending auth mode on the active thread.
pub async fn clear_auth_mode(state: &GatewayState, requested_thread_id: Option<&str>) {
    crate::channels::web::server::clear_auth_mode(state, requested_thread_id).await;
}

pub async fn chat_events_handler(
    State(state): State<Arc<GatewayState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state.sse.subscribe().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Too many connections".to_string(),
    ))
}

pub async fn chat_ws_handler(
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Validate Origin header to prevent cross-site WebSocket hijacking.
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "WebSocket Origin header required".to_string(),
            )
        })?;

    if !is_allowed_local_ws_origin(origin) {
        return Err((
            StatusCode::FORBIDDEN,
            "WebSocket origin not allowed".to_string(),
        ));
    }
    Ok(ws.on_upgrade(move |socket| crate::channels::web::ws::handle_ws_connection(socket, state)))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub thread_id: Option<String>,
    pub limit: Option<usize>,
    pub before: Option<String>,
}

fn is_allowed_local_ws_origin(origin: &str) -> bool {
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn turn_infos_from_thread(thread: &crate::agent::session::Thread) -> Vec<TurnInfo> {
    thread
        .turns
        .iter()
        .map(|t| TurnInfo {
            turn_number: t.turn_number,
            user_input: t.user_input.clone(),
            response: t.response.clone(),
            state: format!("{:?}", t.state),
            started_at: t.started_at.to_rfc3339(),
            completed_at: t.completed_at.map(|dt| dt.to_rfc3339()),
            tool_calls: t
                .tool_calls
                .iter()
                .map(|tc| ToolCallInfo {
                    name: tc.name.clone(),
                    has_result: tc.result.is_some(),
                    has_error: tc.error.is_some(),
                })
                .collect(),
        })
        .collect()
}

pub async fn chat_history_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<HistoryResponse>, (StatusCode, String)> {
    let session_manager = state.session_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Session manager not available".to_string(),
    ))?;

    let limit = query.limit.unwrap_or(50);
    let before_cursor = query
        .before
        .as_deref()
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|_| {
                    (
                        StatusCode::BAD_REQUEST,
                        "Invalid 'before' timestamp".to_string(),
                    )
                })
        })
        .transpose()?;

    let session = session_manager.get_or_create_session(&state.user_id).await;
    let explicit_thread_id = if let Some(ref tid) = query.thread_id {
        Uuid::parse_str(tid)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid thread_id".to_string()))?
    } else {
        Uuid::nil()
    };

    let (thread_id, in_memory_has_thread, in_memory_turns) = {
        let sess = session.lock().await;
        let resolved_thread_id = if query.thread_id.is_some() {
            explicit_thread_id
        } else {
            sess.active_thread
                .ok_or((StatusCode::NOT_FOUND, "No active thread".to_string()))?
        };
        let in_memory_has_thread = sess.threads.contains_key(&resolved_thread_id);
        let in_memory_turns = if before_cursor.is_none() {
            sess.threads
                .get(&resolved_thread_id)
                .filter(|thread| !thread.turns.is_empty())
                .map(turn_infos_from_thread)
        } else {
            None
        };
        (resolved_thread_id, in_memory_has_thread, in_memory_turns)
    };

    // Verify the thread belongs to the authenticated user before returning any data.
    if query.thread_id.is_some()
        && let Some(ref store) = state.store
    {
        let owned = store
            .conversation_belongs_to_user(thread_id, &state.user_id)
            .await
            .unwrap_or(false);
        if !owned && !in_memory_has_thread {
            return Err((StatusCode::NOT_FOUND, "Thread not found".to_string()));
        }
    }

    // For paginated requests (before cursor set), always go to DB
    if before_cursor.is_some()
        && let Some(ref store) = state.store
    {
        let (messages, has_more) = store
            .list_conversation_messages_paginated(thread_id, before_cursor, limit as i64)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let oldest_timestamp = messages.first().map(|m| m.created_at.to_rfc3339());
        let turns = build_turns_from_db_messages(&messages);
        return Ok(Json(HistoryResponse {
            thread_id,
            turns,
            has_more,
            oldest_timestamp,
        }));
    }

    // Try in-memory first (freshest data for active threads)
    if let Some(turns) = in_memory_turns {
        return Ok(Json(HistoryResponse {
            thread_id,
            turns,
            has_more: false,
            oldest_timestamp: None,
        }));
    }

    // Fall back to DB for historical threads not in memory (paginated)
    if let Some(ref store) = state.store {
        let (messages, has_more) = store
            .list_conversation_messages_paginated(thread_id, None, limit as i64)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if !messages.is_empty() {
            let oldest_timestamp = messages.first().map(|m| m.created_at.to_rfc3339());
            let turns = build_turns_from_db_messages(&messages);
            return Ok(Json(HistoryResponse {
                thread_id,
                turns,
                has_more,
                oldest_timestamp,
            }));
        }
    }

    // Empty thread (just created, no messages yet)
    Ok(Json(HistoryResponse {
        thread_id,
        turns: Vec::new(),
        has_more: false,
        oldest_timestamp: None,
    }))
}

/// Build TurnInfo pairs from flat DB messages (alternating user/assistant).
pub fn build_turns_from_db_messages(
    messages: &[crate::history::ConversationMessage],
) -> Vec<TurnInfo> {
    let mut turns = Vec::new();
    let mut turn_number = 0;
    let mut iter = messages.iter().peekable();

    while let Some(msg) = iter.next() {
        if msg.role == "user" {
            let mut turn = TurnInfo {
                turn_number,
                user_input: msg.content.clone(),
                response: None,
                state: "Completed".to_string(),
                started_at: msg.created_at.to_rfc3339(),
                completed_at: None,
                tool_calls: Vec::new(),
            };

            // Check if next message is an assistant response
            if let Some(next) = iter.peek()
                && next.role == "assistant"
            {
                let assistant_msg = iter.next().expect("peeked");
                turn.response = Some(assistant_msg.content.clone());
                turn.completed_at = Some(assistant_msg.created_at.to_rfc3339());
            }

            // Incomplete turn (user message without response)
            if turn.response.is_none() {
                turn.state = "Failed".to_string();
            }

            turns.push(turn);
            turn_number += 1;
        }
    }

    turns
}

pub async fn chat_threads_handler(
    State(state): State<Arc<GatewayState>>,
) -> Result<Json<ThreadListResponse>, (StatusCode, String)> {
    let session_manager = state.session_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Session manager not available".to_string(),
    ))?;

    let session = session_manager.get_or_create_session(&state.user_id).await;
    let sess = session.lock().await;

    // Try DB first for persistent thread list
    if let Some(ref store) = state.store {
        // Auto-create assistant thread if it doesn't exist
        let assistant_id = store
            .get_or_create_assistant_conversation(&state.user_id, "gateway")
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if let Ok(summaries) = store
            .list_conversations_with_preview(&state.user_id, "gateway", 50)
            .await
        {
            let mut assistant_thread = None;
            let mut threads = Vec::new();

            for s in &summaries {
                let info = ThreadInfo {
                    id: s.id,
                    state: "Idle".to_string(),
                    turn_count: (s.message_count / 2).max(0) as usize,
                    created_at: s.started_at.to_rfc3339(),
                    updated_at: s.last_activity.to_rfc3339(),
                    title: s.title.clone(),
                    thread_type: s.thread_type.clone(),
                };

                if s.id == assistant_id {
                    assistant_thread = Some(info);
                } else {
                    threads.push(info);
                }
            }

            // If assistant wasn't in the list (0 messages), synthesize it
            if assistant_thread.is_none() {
                assistant_thread = Some(ThreadInfo {
                    id: assistant_id,
                    state: "Idle".to_string(),
                    turn_count: 0,
                    created_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                    title: None,
                    thread_type: Some("assistant".to_string()),
                });
            }

            return Ok(Json(ThreadListResponse {
                assistant_thread,
                threads,
                active_thread: sess.active_thread,
            }));
        }
    }

    // Fallback: in-memory only (no assistant thread without DB)
    let threads: Vec<ThreadInfo> = sess
        .threads
        .values()
        .map(|t| ThreadInfo {
            id: t.id,
            state: format!("{:?}", t.state),
            turn_count: t.turns.len(),
            created_at: t.created_at.to_rfc3339(),
            updated_at: t.updated_at.to_rfc3339(),
            title: None,
            thread_type: None,
        })
        .collect();

    Ok(Json(ThreadListResponse {
        assistant_thread: None,
        threads,
        active_thread: sess.active_thread,
    }))
}

pub async fn chat_new_thread_handler(
    State(state): State<Arc<GatewayState>>,
) -> Result<Json<ThreadInfo>, (StatusCode, String)> {
    let session_manager = state.session_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Session manager not available".to_string(),
    ))?;

    let session = session_manager.get_or_create_session(&state.user_id).await;
    let mut sess = session.lock().await;
    let thread = sess.create_thread();
    let thread_id = thread.id;
    let info = ThreadInfo {
        id: thread.id,
        state: format!("{:?}", thread.state),
        turn_count: thread.turns.len(),
        created_at: thread.created_at.to_rfc3339(),
        updated_at: thread.updated_at.to_rfc3339(),
        title: None,
        thread_type: Some("thread".to_string()),
    };

    // Persist the empty conversation row with thread_type metadata
    if let Some(ref store) = state.store {
        let store = Arc::clone(store);
        let user_id = state.user_id.clone();
        tokio::spawn(async move {
            if let Err(e) = store
                .ensure_conversation(thread_id, "gateway", &user_id, None)
                .await
            {
                tracing::warn!("Failed to persist new thread: {}", e);
            }
            let metadata_val = serde_json::json!("thread");
            if let Err(e) = store
                .update_conversation_metadata_field(thread_id, "thread_type", &metadata_val)
                .await
            {
                tracing::warn!("Failed to set thread_type metadata: {}", e);
            }
        });
    }

    Ok(Json(info))
}

pub async fn chat_delete_thread_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let thread_id = Uuid::parse_str(&id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid thread ID".to_string()))?;

    let session_manager = state.session_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Session manager not available".to_string(),
    ))?;

    let session = session_manager.get_or_create_session(&state.user_id).await;
    let (in_memory_owned, was_active) = {
        let sess = session.lock().await;
        (
            sess.threads.contains_key(&thread_id),
            sess.active_thread == Some(thread_id),
        )
    };

    let db_owned = if let Some(ref store) = state.store {
        store
            .conversation_belongs_to_user(thread_id, &state.user_id)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    let owned = db_owned || in_memory_owned;
    if !owned {
        return Err((StatusCode::NOT_FOUND, "Thread not found".to_string()));
    }

    let mut deleted = false;
    if let Some(ref store) = state.store {
        deleted = store
            .delete_conversation_for_user(thread_id, &state.user_id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let next_active = if was_active {
        if let Some(ref store) = state.store {
            store
                .get_or_create_assistant_conversation(&state.user_id, "gateway")
                .await
                .ok()
        } else {
            None
        }
    } else {
        None
    };

    let active_thread = {
        let mut sess = session.lock().await;
        sess.threads.remove(&thread_id);
        if was_active {
            sess.active_thread = next_active.or_else(|| sess.threads.keys().copied().next());
        }
        sess.active_thread
    };

    session_manager
        .remove_thread_references(&state.user_id, "gateway", thread_id)
        .await;

    Ok(Json(serde_json::json!({
        "status": "deleted",
        "thread_id": thread_id,
        "deleted": deleted || owned,
        "active_thread": active_thread,
    })))
}

pub async fn chat_clear_threads_handler(
    State(state): State<Arc<GatewayState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let session_manager = state.session_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Session manager not available".to_string(),
    ))?;
    let session = session_manager.get_or_create_session(&state.user_id).await;
    let (cleared_thread_ids, in_memory_count) = {
        let mut sess = session.lock().await;
        let ids = sess.threads.keys().copied().collect::<Vec<_>>();
        let count = ids.len() as u64;
        sess.threads.clear();
        sess.active_thread = None;
        (ids, count)
    };

    let mut deleted_count = 0u64;
    let mut assistant_thread: Option<Uuid> = None;
    if let Some(ref store) = state.store {
        deleted_count = store
            .delete_all_conversations_for_user_channel(&state.user_id, "gateway")
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        assistant_thread = store
            .get_or_create_assistant_conversation(&state.user_id, "gateway")
            .await
            .ok();
    }

    for thread_id in &cleared_thread_ids {
        session_manager
            .remove_thread_references(&state.user_id, "gateway", *thread_id)
            .await;
    }

    if let Some(thread_id) = assistant_thread {
        let mut sess = session.lock().await;
        sess.active_thread = Some(thread_id);
    }

    Ok(Json(serde_json::json!({
        "status": "cleared",
        "deleted_threads": deleted_count.max(in_memory_count),
        "assistant_thread": assistant_thread,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_turns_from_db_messages_complete() {
        let now = chrono::Utc::now();
        let messages = vec![
            crate::history::ConversationMessage {
                id: Uuid::new_v4(),
                role: "user".to_string(),
                content: "Hello".to_string(),
                created_at: now,
            },
            crate::history::ConversationMessage {
                id: Uuid::new_v4(),
                role: "assistant".to_string(),
                content: "Hi there!".to_string(),
                created_at: now + chrono::TimeDelta::seconds(1),
            },
            crate::history::ConversationMessage {
                id: Uuid::new_v4(),
                role: "user".to_string(),
                content: "How are you?".to_string(),
                created_at: now + chrono::TimeDelta::seconds(2),
            },
            crate::history::ConversationMessage {
                id: Uuid::new_v4(),
                role: "assistant".to_string(),
                content: "Doing well!".to_string(),
                created_at: now + chrono::TimeDelta::seconds(3),
            },
        ];

        let turns = build_turns_from_db_messages(&messages);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].user_input, "Hello");
        assert_eq!(turns[0].response.as_deref(), Some("Hi there!"));
        assert_eq!(turns[0].state, "Completed");
        assert_eq!(turns[1].user_input, "How are you?");
        assert_eq!(turns[1].response.as_deref(), Some("Doing well!"));
    }

    #[test]
    fn test_build_turns_from_db_messages_incomplete_last() {
        let now = chrono::Utc::now();
        let messages = vec![
            crate::history::ConversationMessage {
                id: Uuid::new_v4(),
                role: "user".to_string(),
                content: "Hello".to_string(),
                created_at: now,
            },
            crate::history::ConversationMessage {
                id: Uuid::new_v4(),
                role: "assistant".to_string(),
                content: "Hi!".to_string(),
                created_at: now + chrono::TimeDelta::seconds(1),
            },
            crate::history::ConversationMessage {
                id: Uuid::new_v4(),
                role: "user".to_string(),
                content: "Lost message".to_string(),
                created_at: now + chrono::TimeDelta::seconds(2),
            },
        ];

        let turns = build_turns_from_db_messages(&messages);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].user_input, "Lost message");
        assert!(turns[1].response.is_none());
        assert_eq!(turns[1].state, "Failed");
    }
}
