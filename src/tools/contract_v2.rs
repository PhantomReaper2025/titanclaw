//! Tool Contract V2 metadata and reliability profile domain types.
//!
//! Phase 3 introduces a typed metadata layer for tools without breaking the
//! existing `Tool` trait. These types support persisted overrides, resolver
//! outputs, and reliability/ranking state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

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
