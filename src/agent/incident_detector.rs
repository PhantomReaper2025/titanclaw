//! Phase 3 incident detection/deduplication helpers.
//!
//! This module persists structured autonomy incidents from runtime policy/tool
//! events and deduplicates repeated signatures via fingerprint + occurrence
//! increment semantics.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::agent::Incident;
use crate::db::Database;

const INCIDENT_STATUS_OPEN: &str = "open";

#[derive(Debug, Clone)]
pub(crate) struct ToolFailureIncidentRequest {
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub plan_step_id: Option<Uuid>,
    pub execution_attempt_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub user_id: String,
    pub channel: Option<String>,
    pub tool_name: String,
    pub failure_class: Option<String>,
    pub surface: &'static str,
    pub summary: String,
    pub details: Value,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct PolicyDeniedIncidentRequest {
    pub goal_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    pub plan_step_id: Option<Uuid>,
    pub policy_decision_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub user_id: String,
    pub channel: Option<String>,
    pub tool_name: Option<String>,
    pub reason_codes: Vec<String>,
    pub action_kind: String,
    pub surface: &'static str,
    pub summary: String,
    pub details: Value,
    pub observed_at: DateTime<Utc>,
}

fn fingerprint_hex(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(&[0_u8]);
    }
    hasher.finalize().to_hex().to_string()
}

fn severity_for_tool_failure_class(failure_class: Option<&str>) -> &'static str {
    match failure_class {
        Some("tool_timeout")
        | Some("tool_execution")
        | Some("channel")
        | Some("worker")
        | Some("database")
        | Some("orchestrator")
        | Some("tool_sandbox") => "medium",
        Some("tool_not_found")
        | Some("tool_disabled")
        | Some("tool_invalid_parameters")
        | Some("tool_builder_failed")
        | Some("tool_auth_required")
        | Some("safety") => "high",
        _ => "medium",
    }
}

fn severity_for_policy_denial(reason_codes: &[String]) -> &'static str {
    if reason_codes.iter().any(|r| {
        r == "hook_policy_error"
            || r == "hook_rejected"
            || r == "destructive_params_override_auto_approval"
    }) {
        "high"
    } else {
        "medium"
    }
}

async fn create_or_increment_by_fingerprint(store: Arc<dyn Database>, incident: Incident) {
    let observed_at = incident.last_seen_at.unwrap_or(incident.updated_at);
    if let Some(fingerprint) = incident.fingerprint.as_deref() {
        match store
            .find_open_incident_by_fingerprint(&incident.user_id, fingerprint)
            .await
        {
            Ok(Some(existing)) => {
                if let Err(e) = store
                    .increment_incident_occurrence(existing.id, observed_at)
                    .await
                {
                    tracing::warn!(
                        user = %incident.user_id,
                        incident_id = %existing.id,
                        "Failed to increment incident occurrence: {}",
                        e
                    );
                }
                return;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    user = %incident.user_id,
                    "Failed to query open incident by fingerprint: {}",
                    e
                );
            }
        }
    }

    if let Err(e) = store.create_incident(&incident).await {
        tracing::warn!(
            user = %incident.user_id,
            incident_type = %incident.incident_type,
            tool = ?incident.tool_name,
            "Failed to create incident: {}",
            e
        );
    }
}

pub(crate) async fn record_tool_failure_incident_best_effort(
    store: Arc<dyn Database>,
    req: ToolFailureIncidentRequest,
) {
    let observed_at = req.observed_at;
    let failure_class = req.failure_class.clone();
    let fingerprint = fingerprint_hex(&[
        "tool_failure",
        &req.user_id,
        req.surface,
        &req.tool_name,
        req.channel.as_deref().unwrap_or("none"),
        failure_class.as_deref().unwrap_or("none"),
    ]);
    let severity = severity_for_tool_failure_class(failure_class.as_deref()).to_string();

    let incident = Incident {
        id: Uuid::new_v4(),
        goal_id: req.goal_id,
        plan_id: req.plan_id,
        plan_step_id: req.plan_step_id,
        execution_attempt_id: req.execution_attempt_id,
        policy_decision_id: None,
        job_id: req.job_id,
        thread_id: req.thread_id,
        user_id: req.user_id,
        channel: req.channel,
        incident_type: "tool_failure".to_string(),
        severity,
        status: INCIDENT_STATUS_OPEN.to_string(),
        fingerprint: Some(fingerprint),
        surface: Some(req.surface.to_string()),
        tool_name: Some(req.tool_name),
        occurrence_count: 1,
        first_seen_at: Some(observed_at),
        last_seen_at: Some(observed_at),
        last_failure_class: failure_class,
        summary: req.summary,
        details: req.details,
        created_at: observed_at,
        updated_at: observed_at,
        resolved_at: None,
    };

    create_or_increment_by_fingerprint(store, incident).await;
}

pub(crate) async fn record_policy_denied_incident_best_effort(
    store: Arc<dyn Database>,
    req: PolicyDeniedIncidentRequest,
) {
    let observed_at = req.observed_at;
    let mut reason_codes = req.reason_codes.clone();
    reason_codes.sort();
    let reason_signature = if reason_codes.is_empty() {
        "none".to_string()
    } else {
        reason_codes.join(",")
    };
    let fingerprint = fingerprint_hex(&[
        "policy_denial",
        &req.user_id,
        req.surface,
        req.tool_name.as_deref().unwrap_or("none"),
        &req.action_kind,
        &reason_signature,
    ]);
    let severity = severity_for_policy_denial(&reason_codes).to_string();

    let incident = Incident {
        id: Uuid::new_v4(),
        goal_id: req.goal_id,
        plan_id: req.plan_id,
        plan_step_id: req.plan_step_id,
        execution_attempt_id: None,
        policy_decision_id: req.policy_decision_id,
        job_id: req.job_id,
        thread_id: req.thread_id,
        user_id: req.user_id,
        channel: req.channel,
        incident_type: "policy_denial".to_string(),
        severity,
        status: INCIDENT_STATUS_OPEN.to_string(),
        fingerprint: Some(fingerprint),
        surface: Some(req.surface.to_string()),
        tool_name: req.tool_name,
        occurrence_count: 1,
        first_seen_at: Some(observed_at),
        last_seen_at: Some(observed_at),
        last_failure_class: None,
        summary: req.summary,
        details: req.details,
        created_at: observed_at,
        updated_at: observed_at,
        resolved_at: None,
    };

    create_or_increment_by_fingerprint(store, incident).await;
}

#[cfg(test)]
mod tests {
    use super::{fingerprint_hex, severity_for_policy_denial, severity_for_tool_failure_class};

    #[test]
    fn test_fingerprint_is_deterministic() {
        let a = fingerprint_hex(&["tool_failure", "u1", "worker", "shell", "worker"]);
        let b = fingerprint_hex(&["tool_failure", "u1", "worker", "shell", "worker"]);
        let c = fingerprint_hex(&["tool_failure", "u1", "worker", "shell", "web"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_tool_failure_severity_mapping() {
        assert_eq!(
            severity_for_tool_failure_class(Some("tool_timeout")),
            "medium"
        );
        assert_eq!(
            severity_for_tool_failure_class(Some("tool_not_found")),
            "high"
        );
        assert_eq!(severity_for_tool_failure_class(None), "medium");
    }

    #[test]
    fn test_policy_denial_severity_mapping() {
        assert_eq!(
            severity_for_policy_denial(&["hook_rejected".to_string()]),
            "high"
        );
        assert_eq!(
            severity_for_policy_denial(&["user_rejected_tool".to_string()]),
            "medium"
        );
    }
}
