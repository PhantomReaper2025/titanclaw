//! Tool Contract V2 metadata and reliability profile domain types.
//!
//! Phase 3 introduces a typed metadata layer for tools without breaking the
//! existing `Tool` trait. These types support persisted overrides, resolver
//! outputs, and reliability/ranking state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::tool::{Tool, ToolDomain};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolSideEffectLevel {
    ReadOnly,
    WorkspaceWrite,
    SystemMutation,
    ExternalMutation,
    Financial,
    Credential,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolIdempotency {
    SafeRetry,
    UnsafeRetry,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolDryRunSupport {
    None,
    Native,
    Simulated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolContractSource {
    ManualOverride,
    WasmSidecar,
    Inferred,
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContractV2Descriptor {
    pub tool_name: String,
    pub version: i32,
    pub domain: String,
    pub side_effect_level: ToolSideEffectLevel,
    pub idempotency: ToolIdempotency,
    pub dry_run_support: ToolDryRunSupport,
    pub requires_approval_default: bool,
    #[serde(default)]
    pub preconditions: Value,
    #[serde(default)]
    pub postconditions: Value,
    pub timeout_ms_hint: Option<u64>,
    #[serde(default)]
    pub retry_guidance: Value,
    pub compensation_hint: Option<Value>,
    #[serde(default)]
    pub fallback_candidates: Vec<String>,
    #[serde(default)]
    pub risk_tags: Vec<String>,
    pub source: ToolContractSource,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolContractOverrideSource {
    Manual,
    Imported,
    Generated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContractV2Override {
    pub id: Uuid,
    pub tool_name: String,
    pub owner_user_id: Option<String>,
    pub enabled: bool,
    pub descriptor: ToolContractV2Descriptor,
    pub source: ToolContractOverrideSource,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReliabilityProfile {
    pub tool_name: String,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub sample_count: i64,
    pub success_count: i64,
    pub failure_count: i64,
    pub timeout_count: i64,
    pub blocked_count: i64,
    pub success_rate: f32,
    pub p50_latency_ms: Option<i64>,
    pub p95_latency_ms: Option<i64>,
    #[serde(default)]
    pub common_failure_modes: Value,
    pub recent_incident_count: i64,
    pub reliability_score: f32,
    #[serde(default)]
    pub safe_fallback_options: Value,
    pub breaker_state: CircuitBreakerState,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

fn domain_to_string(domain: ToolDomain) -> &'static str {
    match domain {
        ToolDomain::Orchestrator => "orchestrator",
        ToolDomain::Container => "container",
    }
}

fn infer_side_effect_level(
    tool_name: &str,
    domain: ToolDomain,
    requires_approval: bool,
) -> ToolSideEffectLevel {
    match tool_name {
        "echo" | "time" | "json" | "memory_search" | "memory_read" | "memory_graph"
        | "memory_tree" | "read_file" | "list_dir" | "tool_list" | "tool_search" | "skill_list"
        | "skill_search" | "job_status" | "list_jobs" | "job_events" => {
            ToolSideEffectLevel::ReadOnly
        }
        "write_file" | "apply_patch" | "shell" | "build_software" | "job_prompt" => {
            ToolSideEffectLevel::WorkspaceWrite
        }
        "create_job" | "cancel_job" | "tool_install" | "tool_remove" | "tool_activate"
        | "skill_install" | "skill_remove" => ToolSideEffectLevel::SystemMutation,
        "http" | "tool_auth" => ToolSideEffectLevel::ExternalMutation,
        "memory_write" => ToolSideEffectLevel::WorkspaceWrite,
        _ if requires_approval => ToolSideEffectLevel::ExternalMutation,
        _ => match domain {
            ToolDomain::Orchestrator => ToolSideEffectLevel::ReadOnly,
            ToolDomain::Container => ToolSideEffectLevel::WorkspaceWrite,
        },
    }
}

fn infer_idempotency(tool_name: &str, side_effect_level: ToolSideEffectLevel) -> ToolIdempotency {
    match tool_name {
        "echo" | "time" | "json" | "memory_search" | "memory_read" | "memory_graph"
        | "memory_tree" | "read_file" | "list_dir" | "job_status" | "list_jobs" | "job_events" => {
            ToolIdempotency::SafeRetry
        }
        "tool_install" | "tool_remove" | "tool_activate" | "skill_install" | "skill_remove"
        | "create_job" | "cancel_job" => ToolIdempotency::UnsafeRetry,
        "shell" | "http" | "write_file" | "apply_patch" | "memory_write" | "build_software" => {
            ToolIdempotency::Unknown
        }
        _ => match side_effect_level {
            ToolSideEffectLevel::ReadOnly => ToolIdempotency::SafeRetry,
            ToolSideEffectLevel::WorkspaceWrite => ToolIdempotency::Unknown,
            ToolSideEffectLevel::SystemMutation
            | ToolSideEffectLevel::ExternalMutation
            | ToolSideEffectLevel::Financial
            | ToolSideEffectLevel::Credential => ToolIdempotency::UnsafeRetry,
        },
    }
}

fn infer_dry_run_support(tool_name: &str) -> ToolDryRunSupport {
    match tool_name {
        "shell" | "http" => ToolDryRunSupport::Simulated,
        _ => ToolDryRunSupport::None,
    }
}

fn infer_risk_tags(
    tool_name: &str,
    side_effect_level: ToolSideEffectLevel,
    requires_approval: bool,
) -> Vec<String> {
    let mut tags = Vec::new();
    match side_effect_level {
        ToolSideEffectLevel::ReadOnly => tags.push("read_only".to_string()),
        ToolSideEffectLevel::WorkspaceWrite => tags.push("workspace_write".to_string()),
        ToolSideEffectLevel::SystemMutation => tags.push("system_mutation".to_string()),
        ToolSideEffectLevel::ExternalMutation => tags.push("external_mutation".to_string()),
        ToolSideEffectLevel::Financial => tags.push("financial".to_string()),
        ToolSideEffectLevel::Credential => tags.push("credential".to_string()),
    }
    if requires_approval {
        tags.push("approval_required".to_string());
    }
    if matches!(tool_name, "shell" | "http") {
        tags.push("high_variability".to_string());
    }
    tags
}

pub fn infer_tool_contract_v2_descriptor(tool: &dyn Tool) -> ToolContractV2Descriptor {
    let tool_name = tool.name();
    let domain = tool.domain();
    let requires_approval_default = tool.requires_approval();
    let side_effect_level = infer_side_effect_level(tool_name, domain, requires_approval_default);
    let idempotency = infer_idempotency(tool_name, side_effect_level);
    let timeout_ms_hint = Some(
        tool.execution_timeout()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
    );

    let default_max_retries = if matches!(idempotency, ToolIdempotency::SafeRetry) {
        1
    } else {
        0
    };

    ToolContractV2Descriptor {
        tool_name: tool_name.to_string(),
        version: 2,
        domain: domain_to_string(domain).to_string(),
        side_effect_level,
        idempotency,
        dry_run_support: infer_dry_run_support(tool_name),
        requires_approval_default,
        preconditions: serde_json::json!({}),
        postconditions: serde_json::json!({}),
        timeout_ms_hint,
        retry_guidance: serde_json::json!({
            "default_max_retries": default_max_retries,
            "safe_retry": matches!(idempotency, ToolIdempotency::SafeRetry),
        }),
        compensation_hint: None,
        fallback_candidates: Vec::new(),
        risk_tags: infer_risk_tags(tool_name, side_effect_level, requires_approval_default),
        source: ToolContractSource::Inferred,
        updated_at: Utc::now(),
    }
}

pub fn resolve_contract_v2_descriptor_overlay(
    inferred: ToolContractV2Descriptor,
    user_override: Option<&ToolContractV2Override>,
    global_override: Option<&ToolContractV2Override>,
) -> ToolContractV2Descriptor {
    fn apply(record: &ToolContractV2Override) -> Option<ToolContractV2Descriptor> {
        if !record.enabled {
            return None;
        }
        let mut descriptor = record.descriptor.clone();
        descriptor.source = ToolContractSource::ManualOverride;
        Some(descriptor)
    }

    user_override
        .and_then(apply)
        .or_else(|| global_override.and_then(apply))
        .unwrap_or(inferred)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;

    use super::*;
    use crate::context::JobContext;
    use crate::tools::{ToolError, ToolOutput};

    struct TestTool {
        name: &'static str,
        domain: ToolDomain,
        requires_approval: bool,
    }

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "test"
        }

        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type":"object"})
        }

        async fn execute(
            &self,
            _params: Value,
            _ctx: &JobContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::success(
                serde_json::json!({"ok": true}),
                Duration::from_millis(1),
            ))
        }

        fn requires_approval(&self) -> bool {
            self.requires_approval
        }

        fn domain(&self) -> ToolDomain {
            self.domain
        }
    }

    fn sample_override(
        scope_user: Option<&str>,
        enabled: bool,
        source: ToolContractSource,
    ) -> ToolContractV2Override {
        ToolContractV2Override {
            id: Uuid::new_v4(),
            tool_name: "echo".to_string(),
            owner_user_id: scope_user.map(ToString::to_string),
            enabled,
            descriptor: ToolContractV2Descriptor {
                tool_name: "echo".to_string(),
                version: 2,
                domain: "orchestrator".to_string(),
                side_effect_level: ToolSideEffectLevel::ReadOnly,
                idempotency: ToolIdempotency::SafeRetry,
                dry_run_support: ToolDryRunSupport::None,
                requires_approval_default: false,
                preconditions: serde_json::json!({}),
                postconditions: serde_json::json!({}),
                timeout_ms_hint: Some(1000),
                retry_guidance: serde_json::json!({"default_max_retries": 1}),
                compensation_hint: None,
                fallback_candidates: vec![],
                risk_tags: vec!["read_only".to_string()],
                source,
                updated_at: Utc::now(),
            },
            source: ToolContractOverrideSource::Manual,
            notes: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_infer_echo_contract_is_read_only_safe_retry() {
        let tool = TestTool {
            name: "echo",
            domain: ToolDomain::Orchestrator,
            requires_approval: false,
        };
        let descriptor = infer_tool_contract_v2_descriptor(&tool);
        assert_eq!(descriptor.side_effect_level, ToolSideEffectLevel::ReadOnly);
        assert_eq!(descriptor.idempotency, ToolIdempotency::SafeRetry);
        assert_eq!(descriptor.source, ToolContractSource::Inferred);
    }

    #[test]
    fn test_infer_shell_contract_is_workspace_write_unknown_retry() {
        let tool = TestTool {
            name: "shell",
            domain: ToolDomain::Container,
            requires_approval: false,
        };
        let descriptor = infer_tool_contract_v2_descriptor(&tool);
        assert_eq!(
            descriptor.side_effect_level,
            ToolSideEffectLevel::WorkspaceWrite
        );
        assert_eq!(descriptor.idempotency, ToolIdempotency::Unknown);
        assert_eq!(descriptor.dry_run_support, ToolDryRunSupport::Simulated);
    }

    #[test]
    fn test_override_precedence_user_then_global() {
        let inferred = ToolContractV2Descriptor {
            tool_name: "echo".to_string(),
            version: 2,
            domain: "orchestrator".to_string(),
            side_effect_level: ToolSideEffectLevel::ReadOnly,
            idempotency: ToolIdempotency::SafeRetry,
            dry_run_support: ToolDryRunSupport::None,
            requires_approval_default: false,
            preconditions: serde_json::json!({}),
            postconditions: serde_json::json!({}),
            timeout_ms_hint: Some(1000),
            retry_guidance: serde_json::json!({}),
            compensation_hint: None,
            fallback_candidates: vec![],
            risk_tags: vec![],
            source: ToolContractSource::Inferred,
            updated_at: Utc::now(),
        };
        let mut global = sample_override(None, true, ToolContractSource::Default);
        global.descriptor.side_effect_level = ToolSideEffectLevel::ExternalMutation;
        let mut user = sample_override(Some("u1"), true, ToolContractSource::Default);
        user.descriptor.side_effect_level = ToolSideEffectLevel::WorkspaceWrite;

        let resolved = resolve_contract_v2_descriptor_overlay(inferred, Some(&user), Some(&global));
        assert_eq!(
            resolved.side_effect_level,
            ToolSideEffectLevel::WorkspaceWrite
        );
        assert_eq!(resolved.source, ToolContractSource::ManualOverride);
    }

    #[test]
    fn test_disabled_override_falls_back_to_inferred() {
        let tool = TestTool {
            name: "memory_search",
            domain: ToolDomain::Orchestrator,
            requires_approval: false,
        };
        let inferred = infer_tool_contract_v2_descriptor(&tool);
        let disabled = sample_override(Some("u1"), false, ToolContractSource::Default);
        let resolved =
            resolve_contract_v2_descriptor_overlay(inferred.clone(), Some(&disabled), None);
        assert_eq!(resolved.side_effect_level, inferred.side_effect_level);
        assert_eq!(resolved.source, inferred.source);
    }
}
