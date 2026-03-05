//! Execution critic loop helpers (Phase 3).
//!
//! The critic runs deterministic post-step checks and can request replanning
//! when runtime evidence indicates policy drift, unavailable tools, or
//! transient failures that should alter the next plan revision.

use crate::agent::replanner_v1::ReplanReason;
use crate::error::{Error, ToolError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CriticAction {
    Continue,
    Replan { reason: ReplanReason },
}

#[derive(Debug, Clone)]
pub(super) struct CriticDecision {
    pub action: CriticAction,
    pub reason_codes: Vec<String>,
    pub detail: Option<String>,
}

#[derive(Debug)]
pub(super) struct CriticStepInput<'a> {
    pub step_index: usize,
    pub total_steps: usize,
    pub tool_name: &'a str,
    pub elapsed_ms: i64,
    pub result: &'a Result<String, Error>,
}

pub(super) struct ExecutionCritic;

impl ExecutionCritic {
    pub(super) fn evaluate_step(input: CriticStepInput<'_>) -> CriticDecision {
        if let Err(err) = input.result {
            return match err {
                Error::Tool(ToolError::AuthRequired { .. }) => CriticDecision {
                    action: CriticAction::Replan {
                        reason: ReplanReason::PolicyDenied,
                    },
                    reason_codes: vec!["critic_auth_required".to_string()],
                    detail: Some(format!(
                        "Execution critic requested replan after step {}/{} ({}): approval/policy requirement blocked tool execution.",
                        input.step_index + 1,
                        input.total_steps,
                        input.tool_name
                    )),
                },
                Error::Tool(ToolError::Timeout { .. }) => CriticDecision {
                    action: CriticAction::Replan {
                        reason: ReplanReason::StepFailure,
                    },
                    reason_codes: vec!["critic_step_timeout".to_string()],
                    detail: Some(format!(
                        "Execution critic requested replan after step {}/{} ({}): timeout suggests alternate strategy/tool is needed.",
                        input.step_index + 1,
                        input.total_steps,
                        input.tool_name
                    )),
                },
                Error::Tool(ToolError::NotFound { .. }) => CriticDecision {
                    action: CriticAction::Replan {
                        reason: ReplanReason::ToolUnavailable,
                    },
                    reason_codes: vec!["critic_tool_unavailable".to_string()],
                    detail: Some(format!(
                        "Execution critic requested replan after step {}/{} ({}): tool is unavailable.",
                        input.step_index + 1,
                        input.total_steps,
                        input.tool_name
                    )),
                },
                Error::Tool(ToolError::ExecutionFailed { reason, .. })
                    if is_fallback_budget_exhausted_reason(reason) =>
                {
                    CriticDecision {
                        action: CriticAction::Replan {
                            reason: ReplanReason::StepFailure,
                        },
                        reason_codes: vec!["critic_fallback_budget_exhausted".to_string()],
                        detail: Some(format!(
                            "Execution critic requested replan after step {}/{} ({}): {}",
                            input.step_index + 1,
                            input.total_steps,
                            input.tool_name,
                            reason
                        )),
                    }
                }
                Error::Tool(ToolError::ExecutionFailed { reason, .. })
                    if is_policy_block_reason(reason) =>
                {
                    CriticDecision {
                        action: CriticAction::Replan {
                            reason: ReplanReason::PolicyDenied,
                        },
                        reason_codes: vec!["critic_policy_blocked".to_string()],
                        detail: Some(format!(
                            "Execution critic requested replan after step {}/{} ({}): {}",
                            input.step_index + 1,
                            input.total_steps,
                            input.tool_name,
                            reason
                        )),
                    }
                }
                Error::Tool(ToolError::ExecutionFailed { reason, .. })
                    if is_transient_failure_reason(reason) =>
                {
                    CriticDecision {
                        action: CriticAction::Replan {
                            reason: ReplanReason::StepFailure,
                        },
                        reason_codes: vec!["critic_transient_failure".to_string()],
                        detail: Some(format!(
                            "Execution critic requested replan after step {}/{} ({}): transient failure pattern detected ({})",
                            input.step_index + 1,
                            input.total_steps,
                            input.tool_name,
                            reason
                        )),
                    }
                }
                _ => CriticDecision {
                    action: CriticAction::Continue,
                    reason_codes: Vec::new(),
                    detail: None,
                },
            };
        }

        // Long-running successful steps can indicate budget/latency drift.
        if input.elapsed_ms > 60_000 && input.step_index + 1 < input.total_steps {
            return CriticDecision {
                action: CriticAction::Replan {
                    reason: ReplanReason::BudgetExceeded,
                },
                reason_codes: vec!["critic_latency_budget_drift".to_string()],
                detail: Some(format!(
                    "Execution critic requested replan after step {}/{} ({}): step took {}ms and exceeded latency budget.",
                    input.step_index + 1,
                    input.total_steps,
                    input.tool_name,
                    input.elapsed_ms
                )),
            };
        }

        CriticDecision {
            action: CriticAction::Continue,
            reason_codes: Vec::new(),
            detail: None,
        }
    }
}

fn is_policy_block_reason(reason: &str) -> bool {
    let normalized = reason.to_ascii_lowercase();
    normalized.contains("blocked by")
        || normalized.contains("approval")
        || normalized.contains("circuit breaker")
        || normalized.contains("policy")
}

fn is_transient_failure_reason(reason: &str) -> bool {
    let normalized = reason.to_ascii_lowercase();
    normalized.contains("timeout")
        || normalized.contains("temporar")
        || normalized.contains("connection")
        || normalized.contains("network")
        || normalized.contains("rate limit")
}

fn is_fallback_budget_exhausted_reason(reason: &str) -> bool {
    let normalized = reason.to_ascii_lowercase();
    normalized.contains("fallback budget exhausted")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn base_input<'a>(result: &'a Result<String, Error>) -> CriticStepInput<'a> {
        CriticStepInput {
            step_index: 0,
            total_steps: 3,
            tool_name: "shell",
            elapsed_ms: 100,
            result,
        }
    }

    #[test]
    fn critic_maps_circuit_breaker_failure_to_policy_replan() {
        let err: Result<String, Error> = Err(ToolError::ExecutionFailed {
            name: "shell".to_string(),
            reason: "Blocked by reliability circuit breaker".to_string(),
        }
        .into());

        let out = ExecutionCritic::evaluate_step(base_input(&err));
        assert_eq!(
            out.action,
            CriticAction::Replan {
                reason: ReplanReason::PolicyDenied
            }
        );
        assert!(
            out.reason_codes
                .iter()
                .any(|c| c == "critic_policy_blocked")
        );
    }

    #[test]
    fn critic_maps_timeout_to_step_failure_replan() {
        let err: Result<String, Error> = Err(ToolError::Timeout {
            name: "shell".to_string(),
            timeout: Duration::from_secs(5),
        }
        .into());

        let out = ExecutionCritic::evaluate_step(base_input(&err));
        assert_eq!(
            out.action,
            CriticAction::Replan {
                reason: ReplanReason::StepFailure
            }
        );
        assert!(out.reason_codes.iter().any(|c| c == "critic_step_timeout"));
    }

    #[test]
    fn critic_maps_fallback_budget_exhaustion_to_step_failure_replan() {
        let err: Result<String, Error> = Err(ToolError::ExecutionFailed {
            name: "shell".to_string(),
            reason: "Smart-rule fallback budget exhausted after 3 attempt(s); replanning required"
                .to_string(),
        }
        .into());

        let out = ExecutionCritic::evaluate_step(base_input(&err));
        assert_eq!(
            out.action,
            CriticAction::Replan {
                reason: ReplanReason::StepFailure
            }
        );
        assert!(
            out.reason_codes
                .iter()
                .any(|c| c == "critic_fallback_budget_exhausted")
        );
    }

    #[test]
    fn critic_continues_on_quick_success() {
        let ok: Result<String, Error> = Ok("ok".to_string());
        let out = ExecutionCritic::evaluate_step(base_input(&ok));
        assert_eq!(out.action, CriticAction::Continue);
    }

    #[test]
    fn critic_flags_latency_budget_drift_on_long_success_step() {
        let ok: Result<String, Error> = Ok("ok".to_string());
        let out = ExecutionCritic::evaluate_step(CriticStepInput {
            step_index: 0,
            total_steps: 2,
            tool_name: "shell",
            elapsed_ms: 65_000,
            result: &ok,
        });
        assert_eq!(
            out.action,
            CriticAction::Replan {
                reason: ReplanReason::BudgetExceeded
            }
        );
        assert!(
            out.reason_codes
                .iter()
                .any(|c| c == "critic_latency_budget_drift")
        );
    }
}
