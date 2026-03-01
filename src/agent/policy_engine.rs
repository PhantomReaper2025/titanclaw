//! Policy Engine v1 (runtime evaluator, Phase 1).
//!
//! This centralizes approval and hook-based preflight decisions while letting
//! existing call sites retain their current UX and persistence emitters.

use crate::agent::PolicyDecisionKind as AutonomyPolicyDecisionKind;
use crate::agent::autonomy_telemetry::PolicyDecisionKind as TelemetryPolicyDecisionKind;
use crate::hooks::{HookError, HookEvent, HookOutcome, HookRegistry};
use crate::tools::{Tool, ToolContractV2Descriptor, ToolSideEffectLevel};

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
    contract_v2: Option<&ToolContractV2Descriptor>,
) -> ApprovalPolicyOutcome {
    let contract_reason_codes = contract_requires_approval_reason_codes(contract_v2, params);
    let contract_requires_approval = !contract_reason_codes.is_empty();
    if !tool.requires_approval() && !contract_requires_approval {
        return ApprovalPolicyOutcome::NoApprovalRequired;
    }
    if session_auto_approved && !tool.requires_approval_for(params) && !contract_requires_approval {
        return ApprovalPolicyOutcome::AllowAutoApproved {
            reason_codes: vec!["session_auto_approval".to_string()],
        };
    }

    let mut reason_codes = Vec::new();
    if tool.requires_approval() {
        reason_codes.push("tool_requires_approval".to_string());
    }
    reason_codes.extend(contract_reason_codes);
    if session_auto_approved && tool.requires_approval_for(params) {
        reason_codes.push("destructive_params_override_auto_approval".to_string());
    }
    if session_auto_approved && contract_requires_approval {
        reason_codes.push("contract_v2_override_auto_approval".to_string());
    }
    reason_codes.sort();
    reason_codes.dedup();
    ApprovalPolicyOutcome::RequireApproval { reason_codes }
}

pub(super) fn evaluate_worker_tool_approval(
    tool: &dyn Tool,
    params: &serde_json::Value,
    contract_v2: Option<&ToolContractV2Descriptor>,
) -> ApprovalPolicyOutcome {
    let mut reason_codes = Vec::new();
    if tool.requires_approval() {
        reason_codes.push("tool_requires_approval".to_string());
    }
    reason_codes.extend(contract_requires_approval_reason_codes(contract_v2, params));
    reason_codes.sort();
    reason_codes.dedup();

    if reason_codes.is_empty() {
        ApprovalPolicyOutcome::NoApprovalRequired
    } else {
        ApprovalPolicyOutcome::RequireApproval { reason_codes }
    }
}

fn contract_requires_approval_reason_codes(
    contract_v2: Option<&ToolContractV2Descriptor>,
    params: &serde_json::Value,
) -> Vec<String> {
    let Some(contract) = contract_v2 else {
        return Vec::new();
    };

    let dry_run_requested = params_request_dry_run(params);

    let mut reason_codes = Vec::new();
    if contract.requires_approval_default {
        reason_codes.push("contract_v2_requires_approval_default".to_string());
    }
    match contract.side_effect_level {
        ToolSideEffectLevel::ReadOnly | ToolSideEffectLevel::WorkspaceWrite => {}
        ToolSideEffectLevel::SystemMutation => {
            if !dry_run_requested
                || matches!(
                    contract.dry_run_support,
                    crate::tools::ToolDryRunSupport::None
                )
            {
                reason_codes.push("contract_v2_system_mutation".to_string());
            }
        }
        ToolSideEffectLevel::ExternalMutation => {
            if !dry_run_requested
                || matches!(
                    contract.dry_run_support,
                    crate::tools::ToolDryRunSupport::None
                )
            {
                reason_codes.push("contract_v2_external_mutation".to_string());
            }
        }
        ToolSideEffectLevel::Financial => {
            reason_codes.push("contract_v2_financial".to_string());
        }
        ToolSideEffectLevel::Credential => {
            reason_codes.push("contract_v2_credential".to_string());
        }
    }
    if matches!(
        contract.side_effect_level,
        ToolSideEffectLevel::SystemMutation | ToolSideEffectLevel::ExternalMutation
    ) {
        match contract.dry_run_support {
            crate::tools::ToolDryRunSupport::None => {
                reason_codes.push("contract_v2_non_simulatable_high_impact".to_string());
            }
            crate::tools::ToolDryRunSupport::Native
            | crate::tools::ToolDryRunSupport::Simulated
                if !dry_run_requested =>
            {
                reason_codes.push("contract_v2_dry_run_not_requested".to_string());
            }
            _ => {}
        }
    }
    reason_codes
}

fn params_request_dry_run(params: &serde_json::Value) -> bool {
    if params
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    if params
        .get("simulate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    if params
        .get("simulation")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    if params
        .get("preview_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    params
        .get("mode")
        .and_then(|v| v.as_str())
        .map(|s| {
            matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "dry_run" | "simulate"
            )
        })
        .unwrap_or(false)
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
    use crate::tools::{ToolContractSource, ToolDryRunSupport, ToolIdempotency};
    use chrono::Utc;
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
        let out = evaluate_dispatcher_tool_approval(&tool, &serde_json::json!({}), true, None);
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
        let out = evaluate_dispatcher_tool_approval(&tool, &serde_json::json!({}), true, None);
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
        match evaluate_worker_tool_approval(&tool, &serde_json::json!({}), None) {
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

    #[test]
    fn test_dispatcher_contract_v2_requires_explicit_approval_and_blocks_auto_approval() {
        let tool = TestTool {
            needs_approval: false,
            force_explicit_for_params: false,
        };
        let contract = ToolContractV2Descriptor {
            tool_name: "http".to_string(),
            version: 2,
            domain: "orchestrator".to_string(),
            side_effect_level: ToolSideEffectLevel::ExternalMutation,
            idempotency: ToolIdempotency::Unknown,
            dry_run_support: ToolDryRunSupport::Simulated,
            requires_approval_default: false,
            preconditions: serde_json::json!({}),
            postconditions: serde_json::json!({}),
            timeout_ms_hint: Some(30_000),
            retry_guidance: serde_json::json!({}),
            compensation_hint: None,
            fallback_candidates: Vec::new(),
            risk_tags: vec!["external_mutation".to_string()],
            source: ToolContractSource::Inferred,
            updated_at: Utc::now(),
        };

        match evaluate_dispatcher_tool_approval(
            &tool,
            &serde_json::json!({}),
            true,
            Some(&contract),
        ) {
            ApprovalPolicyOutcome::RequireApproval { reason_codes } => {
                assert!(reason_codes.contains(&"contract_v2_external_mutation".to_string()));
                assert!(reason_codes.contains(&"contract_v2_override_auto_approval".to_string()));
            }
            _ => panic!("unexpected outcome"),
        }
    }

    #[test]
    fn test_worker_contract_v2_requires_approval() {
        let tool = TestTool {
            needs_approval: false,
            force_explicit_for_params: false,
        };
        let contract = ToolContractV2Descriptor {
            tool_name: "bank_transfer".to_string(),
            version: 2,
            domain: "orchestrator".to_string(),
            side_effect_level: ToolSideEffectLevel::Financial,
            idempotency: ToolIdempotency::UnsafeRetry,
            dry_run_support: ToolDryRunSupport::None,
            requires_approval_default: true,
            preconditions: serde_json::json!({}),
            postconditions: serde_json::json!({}),
            timeout_ms_hint: Some(10_000),
            retry_guidance: serde_json::json!({}),
            compensation_hint: None,
            fallback_candidates: Vec::new(),
            risk_tags: vec!["financial".to_string()],
            source: ToolContractSource::ManualOverride,
            updated_at: Utc::now(),
        };

        match evaluate_worker_tool_approval(&tool, &serde_json::json!({}), Some(&contract)) {
            ApprovalPolicyOutcome::RequireApproval { reason_codes } => {
                assert!(reason_codes.contains(&"contract_v2_financial".to_string()));
                assert!(
                    reason_codes.contains(&"contract_v2_requires_approval_default".to_string())
                );
            }
            _ => panic!("unexpected outcome"),
        }
    }

    #[test]
    fn test_dispatcher_contract_v2_allows_dry_run_for_simulatable_external_mutation() {
        let tool = TestTool {
            needs_approval: false,
            force_explicit_for_params: false,
        };
        let contract = ToolContractV2Descriptor {
            tool_name: "http".to_string(),
            version: 2,
            domain: "orchestrator".to_string(),
            side_effect_level: ToolSideEffectLevel::ExternalMutation,
            idempotency: ToolIdempotency::Unknown,
            dry_run_support: ToolDryRunSupport::Simulated,
            requires_approval_default: false,
            preconditions: serde_json::json!({}),
            postconditions: serde_json::json!({}),
            timeout_ms_hint: Some(30_000),
            retry_guidance: serde_json::json!({}),
            compensation_hint: None,
            fallback_candidates: Vec::new(),
            risk_tags: vec!["external_mutation".to_string()],
            source: ToolContractSource::Inferred,
            updated_at: Utc::now(),
        };

        match evaluate_dispatcher_tool_approval(
            &tool,
            &serde_json::json!({"dry_run": true}),
            false,
            Some(&contract),
        ) {
            ApprovalPolicyOutcome::NoApprovalRequired => {}
            _ => panic!("expected dry-run external mutation to pass preflight without approval"),
        }
    }

    #[test]
    fn test_dispatcher_contract_v2_flags_non_simulatable_high_impact() {
        let tool = TestTool {
            needs_approval: false,
            force_explicit_for_params: false,
        };
        let contract = ToolContractV2Descriptor {
            tool_name: "system_tool".to_string(),
            version: 2,
            domain: "orchestrator".to_string(),
            side_effect_level: ToolSideEffectLevel::SystemMutation,
            idempotency: ToolIdempotency::Unknown,
            dry_run_support: ToolDryRunSupport::None,
            requires_approval_default: false,
            preconditions: serde_json::json!({}),
            postconditions: serde_json::json!({}),
            timeout_ms_hint: Some(30_000),
            retry_guidance: serde_json::json!({}),
            compensation_hint: None,
            fallback_candidates: Vec::new(),
            risk_tags: vec!["system_mutation".to_string()],
            source: ToolContractSource::Inferred,
            updated_at: Utc::now(),
        };

        match evaluate_dispatcher_tool_approval(
            &tool,
            &serde_json::json!({}),
            false,
            Some(&contract),
        ) {
            ApprovalPolicyOutcome::RequireApproval { reason_codes } => {
                assert!(reason_codes.contains(&"contract_v2_system_mutation".to_string()));
                assert!(
                    reason_codes.contains(&"contract_v2_non_simulatable_high_impact".to_string())
                );
            }
            _ => panic!("unexpected outcome"),
        }
    }
}
