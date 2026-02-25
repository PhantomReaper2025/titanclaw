//! Policy Engine v1 (runtime evaluator, Phase 1).
//!
//! This centralizes approval and hook-based preflight decisions while letting
//! existing call sites retain their current UX and persistence emitters.

use crate::agent::PolicyDecisionKind as AutonomyPolicyDecisionKind;
use crate::agent::autonomy_telemetry::PolicyDecisionKind as TelemetryPolicyDecisionKind;
use crate::hooks::{HookError, HookEvent, HookOutcome, HookRegistry};
use crate::tools::Tool;

#[derive(Debug, Clone)]
pub(super) enum ApprovalPolicyOutcome {
    NoApprovalRequired,
    RequireApproval { reason_codes: Vec<String> },
    AllowAutoApproved { reason_codes: Vec<String> },
}

#[derive(Debug, Clone)]
pub(super) enum HookToolPolicyOutcome {
    Continue {
        parameters: serde_json::Value,
    },
    Modified {
        parameters: serde_json::Value,
        reason_codes: Vec<String>,
    },
    Deny {
        reason: String,
        reason_codes: Vec<String>,
    },
}

pub(super) fn map_policy_decision_kind(
    kind: TelemetryPolicyDecisionKind,
) -> AutonomyPolicyDecisionKind {
    match kind {
        TelemetryPolicyDecisionKind::Allow => AutonomyPolicyDecisionKind::Allow,
        TelemetryPolicyDecisionKind::RequireApproval => AutonomyPolicyDecisionKind::RequireApproval,
        TelemetryPolicyDecisionKind::Deny => AutonomyPolicyDecisionKind::Deny,
        TelemetryPolicyDecisionKind::Modify => AutonomyPolicyDecisionKind::Modify,
        TelemetryPolicyDecisionKind::RequireMoreEvidence => {
            AutonomyPolicyDecisionKind::RequireMoreEvidence
        }
    }
}

pub(super) fn evaluate_dispatcher_tool_approval(
    tool: &dyn Tool,
    params: &serde_json::Value,
    session_auto_approved: bool,
) -> ApprovalPolicyOutcome {
    if !tool.requires_approval() {
        return ApprovalPolicyOutcome::NoApprovalRequired;
    }
    if session_auto_approved && !tool.requires_approval_for(params) {
        return ApprovalPolicyOutcome::AllowAutoApproved {
            reason_codes: vec!["session_auto_approval".to_string()],
        };
    }

    let mut reason_codes = vec!["tool_requires_approval".to_string()];
    if session_auto_approved && tool.requires_approval_for(params) {
        reason_codes.push("destructive_params_override_auto_approval".to_string());
    }
    ApprovalPolicyOutcome::RequireApproval { reason_codes }
}

pub(super) fn evaluate_worker_tool_approval(tool: &dyn Tool) -> ApprovalPolicyOutcome {
    if tool.requires_approval() {
        ApprovalPolicyOutcome::RequireApproval {
            reason_codes: vec!["tool_requires_approval".to_string()],
        }
    } else {
        ApprovalPolicyOutcome::NoApprovalRequired
    }
}

pub(super) async fn evaluate_tool_call_hook(
    hooks: &HookRegistry,
    tool_name: &str,
    parameters: &serde_json::Value,
    user_id: &str,
    context: &str,
) -> HookToolPolicyOutcome {
    let event = HookEvent::ToolCall {
        tool_name: tool_name.to_string(),
        parameters: parameters.clone(),
        user_id: user_id.to_string(),
        context: context.to_string(),
    };

    match hooks.run(&event).await {
        Err(HookError::Rejected { reason }) => HookToolPolicyOutcome::Deny {
            reason,
            reason_codes: vec!["hook_rejected".to_string()],
        },
        Err(err) => HookToolPolicyOutcome::Deny {
            reason: format!("Hook policy error: {}", err),
            reason_codes: vec!["hook_policy_error".to_string()],
        },
        Ok(HookOutcome::Continue {
            modified: Some(new_params),
        }) => match serde_json::from_str::<serde_json::Value>(&new_params) {
            Ok(parsed) => HookToolPolicyOutcome::Modified {
                parameters: parsed,
                reason_codes: vec!["hook_modified_parameters".to_string()],
            },
            Err(e) => {
                tracing::warn!(
                    tool = %tool_name,
                    "Hook returned non-JSON modification for ToolCall, ignoring: {}",
                    e
                );
                HookToolPolicyOutcome::Continue {
                    parameters: parameters.clone(),
                }
            }
        },
        _ => HookToolPolicyOutcome::Continue {
            parameters: parameters.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct TestTool {
        needs_approval: bool,
        force_explicit_for_params: bool,
    }

    #[async_trait::async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "test"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _job_ctx: &crate::context::JobContext,
        ) -> Result<crate::tools::ToolOutput, crate::tools::ToolError> {
            unreachable!("not used in policy tests")
        }
        fn requires_approval(&self) -> bool {
            self.needs_approval
        }
        fn requires_approval_for(&self, _params: &serde_json::Value) -> bool {
            self.force_explicit_for_params
        }
        fn execution_timeout(&self) -> Duration {
            Duration::from_secs(1)
        }
    }

    #[test]
    fn test_dispatcher_approval_auto_approved() {
        let tool = TestTool {
            needs_approval: true,
            force_explicit_for_params: false,
        };
        let out = evaluate_dispatcher_tool_approval(&tool, &serde_json::json!({}), true);
        match out {
            ApprovalPolicyOutcome::AllowAutoApproved { reason_codes } => {
                assert_eq!(reason_codes, vec!["session_auto_approval"]);
            }
            _ => panic!("unexpected outcome"),
        }
    }

    #[test]
    fn test_dispatcher_approval_requires_explicit_on_destructive_override() {
        let tool = TestTool {
            needs_approval: true,
            force_explicit_for_params: true,
        };
        let out = evaluate_dispatcher_tool_approval(&tool, &serde_json::json!({}), true);
        match out {
            ApprovalPolicyOutcome::RequireApproval { reason_codes } => {
                assert!(
                    reason_codes.contains(&"destructive_params_override_auto_approval".to_string())
                );
            }
            _ => panic!("unexpected outcome"),
        }
    }

    #[test]
    fn test_worker_approval_requires_approval() {
        let tool = TestTool {
            needs_approval: true,
            force_explicit_for_params: false,
        };
        match evaluate_worker_tool_approval(&tool) {
            ApprovalPolicyOutcome::RequireApproval { reason_codes } => {
                assert_eq!(reason_codes, vec!["tool_requires_approval"]);
            }
            _ => panic!("unexpected outcome"),
        }
    }

    #[test]
    fn test_map_policy_decision_kind_supports_require_more_evidence() {
        let mapped = map_policy_decision_kind(TelemetryPolicyDecisionKind::RequireMoreEvidence);
        assert_eq!(mapped, AutonomyPolicyDecisionKind::RequireMoreEvidence);
    }
}
