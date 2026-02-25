//! Structured telemetry records for autonomy/policy decisions.
//!
//! This is an internal bridge toward a fuller autonomy control plane. It emits
//! typed records via tracing without changing runtime behavior.

use std::time::Duration;

use serde::Serialize;
use uuid::Uuid;

use crate::error::{Error, ToolError};

const ERROR_PREVIEW_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PolicyDecisionKind {
    Allow,
    RequireApproval,
    Deny,
    Modify,
    RequireMoreEvidence,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PolicyDecisionRecord {
    pub user_id: String,
    pub channel: String,
    pub thread_id: Uuid,
    pub tool_name: String,
    pub tool_call_id: String,
    pub decision: PolicyDecisionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reason_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_approved: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExecutionAttemptStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FailureClass {
    ToolTimeout,
    ToolInvalidParameters,
    ToolExecution,
    ToolSandbox,
    ToolDisabled,
    ToolAuthRequired,
    ToolNotFound,
    ToolBuilderFailed,
    Safety,
    Channel,
    Llm,
    Job,
    Database,
    Workspace,
    Orchestrator,
    Worker,
    Repair,
    Config,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ExecutionAttemptRecord {
    pub user_id: String,
    pub channel: String,
    pub thread_id: Uuid,
    pub tool_name: String,
    pub tool_call_id: String,
    pub live_stream: bool,
    pub piped_cache_hit: bool,
    pub elapsed_ms: u64,
    pub status: ExecutionAttemptStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<FailureClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_preview: Option<String>,
}

pub(super) fn emit_policy_decision(record: &PolicyDecisionRecord) {
    match serde_json::to_string(record) {
        Ok(payload) => tracing::info!(
            target: "autonomy.policy",
            event_type = "policy_decision",
            payload = %payload,
            "Structured autonomy telemetry"
        ),
        Err(error) => tracing::warn!(
            target: "autonomy.policy",
            event_type = "policy_decision",
            error = %error,
            "Failed to serialize autonomy telemetry record"
        ),
    }
}

pub(super) fn emit_execution_attempt(record: &ExecutionAttemptRecord) {
    match serde_json::to_string(record) {
        Ok(payload) => tracing::info!(
            target: "autonomy.execution",
            event_type = "execution_attempt",
            payload = %payload,
            "Structured autonomy telemetry"
        ),
        Err(error) => tracing::warn!(
            target: "autonomy.execution",
            event_type = "execution_attempt",
            error = %error,
            "Failed to serialize autonomy telemetry record"
        ),
    }
}

pub(super) fn classify_failure(err: &Error) -> FailureClass {
    match err {
        Error::Tool(tool_err) => match tool_err {
            ToolError::Timeout { .. } => FailureClass::ToolTimeout,
            ToolError::InvalidParameters { .. } => FailureClass::ToolInvalidParameters,
            ToolError::ExecutionFailed { .. } => FailureClass::ToolExecution,
            ToolError::Sandbox { .. } => FailureClass::ToolSandbox,
            ToolError::Disabled { .. } => FailureClass::ToolDisabled,
            ToolError::AuthRequired { .. } => FailureClass::ToolAuthRequired,
            ToolError::NotFound { .. } => FailureClass::ToolNotFound,
            ToolError::BuilderFailed(_) => FailureClass::ToolBuilderFailed,
        },
        Error::Safety(_) => FailureClass::Safety,
        Error::Channel(_) => FailureClass::Channel,
        Error::Llm(_) => FailureClass::Llm,
        Error::Job(_) => FailureClass::Job,
        Error::Database(_) => FailureClass::Database,
        Error::Workspace(_) => FailureClass::Workspace,
        Error::Orchestrator(_) => FailureClass::Orchestrator,
        Error::Worker(_) => FailureClass::Worker,
        Error::Repair(_) => FailureClass::Repair,
        Error::Config(_) => FailureClass::Config,
        _ => FailureClass::Other,
    }
}

pub(super) fn truncate_error_preview(err: &Error) -> String {
    truncate_chars(&err.to_string(), ERROR_PREVIEW_CHARS)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated = s.chars().take(max_chars).collect::<String>();
    format!("{}…", truncated)
}

pub(super) fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ExecutionAttemptRecord, ExecutionAttemptStatus, FailureClass, PolicyDecisionKind,
        PolicyDecisionRecord, classify_failure, elapsed_ms, truncate_error_preview,
    };
    use crate::error::{Error, ToolError};
    use uuid::Uuid;

    #[test]
    fn test_classify_failure_timeout() {
        let err: Error = ToolError::Timeout {
            name: "shell".to_string(),
            timeout: Duration::from_secs(5),
        }
        .into();

        assert_eq!(classify_failure(&err), FailureClass::ToolTimeout);
    }

    #[test]
    fn test_truncate_error_preview_limits_length() {
        let long_reason = "x".repeat(400);
        let err: Error = ToolError::ExecutionFailed {
            name: "shell".to_string(),
            reason: long_reason,
        }
        .into();
        let preview = truncate_error_preview(&err);

        assert!(preview.chars().count() <= 241);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn test_policy_decision_serialization_skips_none() {
        let record = PolicyDecisionRecord {
            user_id: "u".to_string(),
            channel: "cli".to_string(),
            thread_id: Uuid::nil(),
            tool_name: "shell".to_string(),
            tool_call_id: "call-1".to_string(),
            decision: PolicyDecisionKind::Allow,
            reason_codes: vec!["session_auto_approval".to_string()],
            auto_approved: None,
        };

        let json = serde_json::to_value(&record).expect("serialize");
        assert!(json.get("auto_approved").is_none());
        assert_eq!(json["decision"], "allow");
    }

    #[test]
    fn test_execution_attempt_serialization_includes_failure_fields() {
        let record = ExecutionAttemptRecord {
            user_id: "u".to_string(),
            channel: "cli".to_string(),
            thread_id: Uuid::nil(),
            tool_name: "shell".to_string(),
            tool_call_id: "call-1".to_string(),
            live_stream: true,
            piped_cache_hit: false,
            elapsed_ms: elapsed_ms(Duration::from_millis(12)),
            status: ExecutionAttemptStatus::Failed,
            failure_class: Some(FailureClass::ToolExecution),
            error_preview: Some("boom".to_string()),
        };

        let json = serde_json::to_value(&record).expect("serialize");
        assert_eq!(json["status"], "failed");
        assert_eq!(json["failure_class"], "tool_execution");
        assert_eq!(json["elapsed_ms"], 12);
    }
}
