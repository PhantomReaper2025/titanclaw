//! Tool reliability profile aggregation and manual recompute service (Phase 3).
//!
//! This module is intentionally conservative: it computes and persists
//! reliability profiles from existing persisted autonomy execution/policy/
//! incident records without changing runtime routing behavior yet.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use crate::agent::{
    ExecutionAttempt, ExecutionAttemptStatus, Incident, PolicyDecision, PolicyDecisionKind,
};
use crate::db::Database;
use crate::error::DatabaseError;
use crate::tools::{CircuitBreakerState, ToolReliabilityProfile};

const DEFAULT_WINDOW_MINUTES: i64 = 24 * 60;
const DEFAULT_MIN_SAMPLES_FOR_CONFIDENCE: i64 = 5;
const DEFAULT_BREAKER_OPEN_FAILURES: i64 = 3;
const DEFAULT_BREAKER_OPEN_WINDOW_MINUTES: i64 = 10;
const DEFAULT_BREAKER_COOLDOWN_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub(crate) struct ToolReliabilityConfig {
    pub window: Duration,
    pub min_samples_for_confidence: i64,
    pub breaker_open_failures: i64,
    pub breaker_open_window: Duration,
    pub breaker_cooldown: Duration,
}

impl Default for ToolReliabilityConfig {
    fn default() -> Self {
        Self {
            window: Duration::minutes(DEFAULT_WINDOW_MINUTES),
            min_samples_for_confidence: DEFAULT_MIN_SAMPLES_FOR_CONFIDENCE,
            breaker_open_failures: DEFAULT_BREAKER_OPEN_FAILURES,
            breaker_open_window: Duration::minutes(DEFAULT_BREAKER_OPEN_WINDOW_MINUTES),
            breaker_cooldown: Duration::seconds(DEFAULT_BREAKER_COOLDOWN_SECS),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ToolReliabilityService {
    store: Arc<dyn Database>,
    config: ToolReliabilityConfig,
}

impl ToolReliabilityService {
    pub(crate) fn new(store: Arc<dyn Database>) -> Self {
        Self {
            store,
            config: ToolReliabilityConfig::default(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_config(mut self, config: ToolReliabilityConfig) -> Self {
        self.config = config;
        self
    }

    pub(crate) async fn recompute_profile_for_tool(
        &self,
        user_id: &str,
        tool_name: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<ToolReliabilityProfile>, DatabaseError> {
        let previous = self.store.get_tool_reliability_profile(tool_name).await?;
        let attempts = self.store.list_execution_attempts_for_user(user_id).await?;
        let policy_decisions = self.store.list_policy_decisions_for_user(user_id).await?;
        let incidents = self.store.list_incidents_for_user(user_id).await?;
        let broken_tools = self.store.get_broken_tools(1).await.unwrap_or_default();
        let legacy_failure_count = broken_tools
            .iter()
            .find(|t| t.name == tool_name)
            .map(|t| i64::from(t.failure_count))
            .unwrap_or(0);

        let profile = compute_tool_reliability_profile(
            tool_name,
            now,
            &attempts,
            &policy_decisions,
            &incidents,
            previous.as_ref(),
            legacy_failure_count,
            &self.config,
        )?;

        let Some(profile) = profile else {
            return Ok(None);
        };

        self.store.upsert_tool_reliability_profile(&profile).await?;
        Ok(Some(profile))
    }
}

fn within_window(ts: DateTime<Utc>, start: DateTime<Utc>) -> bool {
    ts >= start
}

fn is_attempt_for_tool(attempt: &ExecutionAttempt, tool_name: &str) -> bool {
    attempt.tool_name == tool_name
}

fn is_policy_for_tool(decision: &PolicyDecision, tool_name: &str) -> bool {
    decision.tool_name.as_deref() == Some(tool_name)
}

fn is_incident_for_tool(incident: &Incident, tool_name: &str) -> bool {
    incident.tool_name.as_deref() == Some(tool_name)
}

fn failure_class_weighted_transient(failure_class: Option<&str>) -> bool {
    matches!(
        failure_class,
        Some("tool_timeout")
            | Some("tool_execution")
            | Some("channel")
            | Some("orchestrator")
            | Some("worker")
            | Some("database")
    )
}

fn percentile(sorted: &[i64], p: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted.get(idx).copied()
}

fn compute_latency_score(p95_latency_ms: Option<i64>) -> f32 {
    let Some(p95) = p95_latency_ms else {
        return 1.0;
    };
    if p95 <= 0 {
        return 1.0;
    }
    // Conservative heuristic until contract timeout hints are wired in slice 5+.
    // 0ms -> 1.0, 60s+ -> ~0.0, linear clamp.
    let normalized = 1.0 - ((p95 as f32) / 60_000.0);
    normalized.clamp(0.0, 1.0)
}

fn compute_policy_friction_score(decisions: &[&PolicyDecision]) -> f32 {
    if decisions.is_empty() {
        return 1.0;
    }

    let mut deny_like = 0_i64;
    let mut approval_like = 0_i64;
    for d in decisions {
        if matches!(
            d.decision,
            PolicyDecisionKind::Deny | PolicyDecisionKind::RequireMoreEvidence
        ) {
            deny_like += 1;
        } else if matches!(d.decision, PolicyDecisionKind::RequireApproval) {
            approval_like += 1;
        }
    }

    let total = decisions.len() as f32;
    let deny_penalty = (deny_like as f32) / total;
    let approval_penalty = (approval_like as f32) / total * 0.5;
    (1.0 - deny_penalty - approval_penalty).clamp(0.0, 1.0)
}

fn compute_incident_penalty_score(recent_incident_count: i64) -> f32 {
    // Smoothly decays; 0 incidents = 1.0, 5+ incidents trends toward 0.
    (1.0 / (1.0 + recent_incident_count as f32)).clamp(0.0, 1.0)
}

fn common_failure_modes_json(attempts: &[&ExecutionAttempt]) -> serde_json::Value {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for attempt in attempts {
        if let Some(fc) = attempt.failure_class.as_ref() {
            *counts.entry(fc.clone()).or_insert(0) += 1;
        } else if matches!(
            attempt.status,
            ExecutionAttemptStatus::Failed
                | ExecutionAttemptStatus::Timeout
                | ExecutionAttemptStatus::Blocked
        ) {
            *counts.entry("unknown".to_string()).or_insert(0) += 1;
        }
    }

    let mut entries: Vec<(String, i64)> = counts.into_iter().collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    serde_json::json!(
        entries
            .into_iter()
            .take(5)
            .map(|(failure_class, count)| serde_json::json!({"failure_class": failure_class, "count": count}))
            .collect::<Vec<_>>()
    )
}

fn classify_transient_failures_in_window(
    now: DateTime<Utc>,
    attempts: &[&ExecutionAttempt],
    breaker_open_window: Duration,
) -> i64 {
    let start = now - breaker_open_window;
    attempts
        .iter()
        .filter(|a| within_window(a.started_at, start))
        .filter(|a| {
            matches!(
                a.status,
                ExecutionAttemptStatus::Failed
                    | ExecutionAttemptStatus::Timeout
                    | ExecutionAttemptStatus::Blocked
            )
        })
        .filter(|a| {
            matches!(a.status, ExecutionAttemptStatus::Timeout)
                || failure_class_weighted_transient(a.failure_class.as_deref())
        })
        .count() as i64
}

fn latest_success_after(
    attempts: &[&ExecutionAttempt],
    ts: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let threshold = ts?;
    attempts
        .iter()
        .filter(|a| a.started_at > threshold)
        .filter(|a| matches!(a.status, ExecutionAttemptStatus::Succeeded))
        .map(|a| a.started_at)
        .max()
}

pub(crate) fn compute_tool_reliability_profile(
    tool_name: &str,
    now: DateTime<Utc>,
    attempts_all: &[ExecutionAttempt],
    policy_decisions_all: &[PolicyDecision],
    incidents_all: &[Incident],
    previous_profile: Option<&ToolReliabilityProfile>,
    legacy_failure_count: i64,
    config: &ToolReliabilityConfig,
) -> Result<Option<ToolReliabilityProfile>, DatabaseError> {
    let window_start = now - config.window;

    let attempts: Vec<&ExecutionAttempt> = attempts_all
        .iter()
        .filter(|a| is_attempt_for_tool(a, tool_name))
        .filter(|a| within_window(a.started_at, window_start))
        .collect();

    if attempts.is_empty() && legacy_failure_count <= 0 {
        return Ok(None);
    }

    let policy_decisions: Vec<&PolicyDecision> = policy_decisions_all
        .iter()
        .filter(|d| is_policy_for_tool(d, tool_name))
        .filter(|d| within_window(d.created_at, window_start))
        .collect();

    let incidents: Vec<&Incident> = incidents_all
        .iter()
        .filter(|i| is_incident_for_tool(i, tool_name))
        .filter(|i| {
            let observed = i.last_seen_at.or(i.first_seen_at).unwrap_or(i.created_at);
            within_window(observed, window_start)
        })
        .collect();

    let sample_count = attempts.len() as i64;
    let mut success_count = 0_i64;
    let mut failure_count = 0_i64;
    let mut timeout_count = 0_i64;
    let mut blocked_count = 0_i64;
    let mut latencies = Vec::new();
    let mut last_failure_at: Option<DateTime<Utc>> = None;
    let mut last_success_at: Option<DateTime<Utc>> = None;

    for attempt in &attempts {
        if let Some(elapsed) = attempt.elapsed_ms
            && elapsed >= 0
        {
            latencies.push(elapsed);
        }

        match attempt.status {
            ExecutionAttemptStatus::Succeeded => {
                success_count += 1;
                last_success_at = Some(match last_success_at {
                    Some(current) => current.max(attempt.started_at),
                    None => attempt.started_at,
                });
            }
            ExecutionAttemptStatus::Timeout => {
                timeout_count += 1;
                failure_count += 1;
                last_failure_at = Some(match last_failure_at {
                    Some(current) => current.max(attempt.started_at),
                    None => attempt.started_at,
                });
            }
            ExecutionAttemptStatus::Blocked => {
                blocked_count += 1;
                last_failure_at = Some(match last_failure_at {
                    Some(current) => current.max(attempt.started_at),
                    None => attempt.started_at,
                });
            }
            ExecutionAttemptStatus::Failed => {
                failure_count += 1;
                if matches!(attempt.failure_class.as_deref(), Some("tool_timeout")) {
                    timeout_count += 1;
                }
                last_failure_at = Some(match last_failure_at {
                    Some(current) => current.max(attempt.started_at),
                    None => attempt.started_at,
                });
            }
            ExecutionAttemptStatus::Running => {}
        }
    }

    latencies.sort_unstable();
    let p50_latency_ms = percentile(&latencies, 0.50);
    let p95_latency_ms = percentile(&latencies, 0.95);

    let recent_incident_count = incidents
        .iter()
        .filter(|i| !matches!(i.status.as_str(), "resolved" | "ignored"))
        .count() as i64;

    let success_rate = if sample_count > 0 {
        (success_count as f32 / sample_count as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let latency_score = compute_latency_score(p95_latency_ms);
    let incident_penalty_score =
        compute_incident_penalty_score(recent_incident_count + legacy_failure_count);
    let policy_friction_score = compute_policy_friction_score(&policy_decisions);

    let mut reliability_score = (0.45 * success_rate)
        + (0.25 * latency_score)
        + (0.20 * incident_penalty_score)
        + (0.10 * policy_friction_score);

    if sample_count < config.min_samples_for_confidence {
        reliability_score *= 0.7;
    }
    if legacy_failure_count > 0 {
        reliability_score *= (1.0 / (1.0 + legacy_failure_count as f32 * 0.15)).clamp(0.2, 1.0);
    }
    reliability_score = reliability_score.clamp(0.0, 1.0);

    let transient_failures_recent =
        classify_transient_failures_in_window(now, &attempts, config.breaker_open_window);
    let has_success_after_last_failure = latest_success_after(&attempts, last_failure_at).is_some();

    let (breaker_state, cooldown_until) = if transient_failures_recent
        >= config.breaker_open_failures
        && !has_success_after_last_failure
    {
        (
            CircuitBreakerState::Open,
            Some(now + config.breaker_cooldown),
        )
    } else if let Some(prev) = previous_profile {
        if matches!(prev.breaker_state, CircuitBreakerState::Open)
            && prev.cooldown_until.is_some()
            && prev.cooldown_until <= Some(now)
        {
            (CircuitBreakerState::HalfOpen, None)
        } else {
            (CircuitBreakerState::Closed, None)
        }
    } else {
        (CircuitBreakerState::Closed, None)
    };

    let safe_fallback_options = serde_json::json!([]);

    Ok(Some(ToolReliabilityProfile {
        tool_name: tool_name.to_string(),
        window_start,
        window_end: now,
        sample_count,
        success_count,
        failure_count,
        timeout_count,
        blocked_count,
        success_rate,
        p50_latency_ms,
        p95_latency_ms,
        common_failure_modes: common_failure_modes_json(&attempts),
        recent_incident_count,
        reliability_score,
        safe_fallback_options,
        breaker_state,
        cooldown_until,
        last_failure_at,
        last_success_at,
        updated_at: now,
    }))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::agent::{ExecutionAttemptStatus, PolicyDecisionKind};

    fn ts(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(sec, 0).single().unwrap()
    }

    fn attempt(
        tool_name: &str,
        started_at: DateTime<Utc>,
        status: ExecutionAttemptStatus,
        failure_class: Option<&str>,
        elapsed_ms: Option<i64>,
    ) -> ExecutionAttempt {
        ExecutionAttempt {
            id: Uuid::new_v4(),
            goal_id: None,
            plan_id: None,
            plan_step_id: None,
            job_id: None,
            thread_id: None,
            user_id: "u1".to_string(),
            channel: "cli".to_string(),
            tool_name: tool_name.to_string(),
            tool_call_id: None,
            tool_args: None,
            status,
            failure_class: failure_class.map(ToString::to_string),
            retry_count: 0,
            started_at,
            finished_at: None,
            elapsed_ms,
            result_summary: None,
            error_preview: None,
        }
    }

    fn policy(
        tool_name: &str,
        created_at: DateTime<Utc>,
        decision: PolicyDecisionKind,
    ) -> PolicyDecision {
        PolicyDecision {
            id: Uuid::new_v4(),
            goal_id: None,
            plan_id: None,
            plan_step_id: None,
            execution_attempt_id: None,
            user_id: "u1".to_string(),
            channel: "cli".to_string(),
            tool_name: Some(tool_name.to_string()),
            tool_call_id: None,
            action_kind: "tool_call".to_string(),
            decision,
            reason_codes: vec![],
            risk_score: None,
            confidence: None,
            requires_approval: false,
            auto_approved: None,
            evidence_required: json!({}),
            created_at,
        }
    }

    fn incident(tool_name: &str, created_at: DateTime<Utc>, status: &str) -> Incident {
        Incident {
            id: Uuid::new_v4(),
            goal_id: None,
            plan_id: None,
            plan_step_id: None,
            execution_attempt_id: None,
            policy_decision_id: None,
            job_id: None,
            thread_id: None,
            user_id: "u1".to_string(),
            channel: Some("cli".to_string()),
            incident_type: "tool_transient".to_string(),
            severity: "medium".to_string(),
            status: status.to_string(),
            fingerprint: Some(format!("f:{}", tool_name)),
            surface: Some("worker".to_string()),
            tool_name: Some(tool_name.to_string()),
            occurrence_count: 1,
            first_seen_at: Some(created_at),
            last_seen_at: Some(created_at),
            last_failure_class: Some("tool_timeout".to_string()),
            summary: "timeout".to_string(),
            details: json!({}),
            created_at,
            updated_at: created_at,
            resolved_at: None,
        }
    }

    #[test]
    fn computes_profile_counts_and_percentiles() {
        let now = ts(10_000);
        let attempts = vec![
            attempt(
                "shell",
                ts(9_900),
                ExecutionAttemptStatus::Succeeded,
                None,
                Some(100),
            ),
            attempt(
                "shell",
                ts(9_910),
                ExecutionAttemptStatus::Failed,
                Some("tool_execution"),
                Some(200),
            ),
            attempt(
                "shell",
                ts(9_920),
                ExecutionAttemptStatus::Timeout,
                Some("tool_timeout"),
                Some(300),
            ),
            attempt(
                "other",
                ts(9_930),
                ExecutionAttemptStatus::Succeeded,
                None,
                Some(50),
            ),
        ];
        let policies = vec![policy(
            "shell",
            ts(9_940),
            PolicyDecisionKind::RequireApproval,
        )];
        let incidents = vec![incident("shell", ts(9_950), "open")];
        let config = ToolReliabilityConfig::default();

        let profile = compute_tool_reliability_profile(
            "shell", now, &attempts, &policies, &incidents, None, 0, &config,
        )
        .unwrap()
        .unwrap();

        assert_eq!(profile.sample_count, 3);
        assert_eq!(profile.success_count, 1);
        assert_eq!(profile.failure_count, 2);
        assert_eq!(profile.timeout_count, 1);
        assert_eq!(profile.blocked_count, 0);
        assert_eq!(profile.p50_latency_ms, Some(200));
        assert_eq!(profile.p95_latency_ms, Some(300));
        assert!(profile.reliability_score >= 0.0 && profile.reliability_score <= 1.0);
    }

    #[test]
    fn opens_breaker_on_repeated_transient_failures() {
        let now = ts(20_000);
        let attempts = vec![
            attempt(
                "http",
                now - Duration::minutes(1),
                ExecutionAttemptStatus::Timeout,
                Some("tool_timeout"),
                Some(1_000),
            ),
            attempt(
                "http",
                now - Duration::minutes(2),
                ExecutionAttemptStatus::Failed,
                Some("tool_execution"),
                Some(800),
            ),
            attempt(
                "http",
                now - Duration::minutes(3),
                ExecutionAttemptStatus::Failed,
                Some("channel"),
                Some(600),
            ),
        ];
        let config = ToolReliabilityConfig::default();

        let profile =
            compute_tool_reliability_profile("http", now, &attempts, &[], &[], None, 0, &config)
                .unwrap()
                .unwrap();

        assert_eq!(profile.breaker_state, CircuitBreakerState::Open);
        assert!(profile.cooldown_until.is_some());
    }

    #[test]
    fn moves_open_breaker_to_half_open_after_cooldown_without_new_failures() {
        let now = ts(30_000);
        let attempts = vec![attempt(
            "shell",
            now - Duration::minutes(30),
            ExecutionAttemptStatus::Succeeded,
            None,
            Some(20),
        )];
        let previous = ToolReliabilityProfile {
            tool_name: "shell".to_string(),
            window_start: now - Duration::hours(1),
            window_end: now - Duration::minutes(5),
            sample_count: 1,
            success_count: 0,
            failure_count: 1,
            timeout_count: 0,
            blocked_count: 0,
            success_rate: 0.0,
            p50_latency_ms: None,
            p95_latency_ms: None,
            common_failure_modes: json!([]),
            recent_incident_count: 0,
            reliability_score: 0.0,
            safe_fallback_options: json!([]),
            breaker_state: CircuitBreakerState::Open,
            cooldown_until: Some(now - Duration::seconds(1)),
            last_failure_at: Some(now - Duration::minutes(10)),
            last_success_at: None,
            updated_at: now - Duration::minutes(10),
        };
        let config = ToolReliabilityConfig::default();

        let profile = compute_tool_reliability_profile(
            "shell",
            now,
            &attempts,
            &[],
            &[],
            Some(&previous),
            0,
            &config,
        )
        .unwrap()
        .unwrap();
        assert_eq!(profile.breaker_state, CircuitBreakerState::HalfOpen);
    }
}
