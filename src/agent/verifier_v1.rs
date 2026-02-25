//! Verifier v1 (Phase 1 soft completion gate for planned worker execution).

use chrono::Utc;

use crate::agent::{Evidence, EvidenceKind, GoalRiskClass, PlanVerificationStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerificationNextAction {
    Complete,
    Replan,
    AskUser,
    GatherEvidence,
    MarkBlocked,
}

#[derive(Debug, Clone)]
pub(super) struct VerificationRequest {
    pub completion_path: &'static str,
    pub completion_claimed: bool,
    pub completion_candidate_summary: String,
    pub planned_steps: usize,
    pub executed_steps: usize,
    pub succeeded_steps: usize,
    pub failed_steps: usize,
    pub step_outcomes: Vec<serde_json::Value>,
    pub base_evidence: Vec<Evidence>,
    pub acceptance_criteria: serde_json::Value,
    pub risk_class: GoalRiskClass,
}

#[derive(Debug, Clone)]
pub(super) struct VerificationResult {
    pub status: PlanVerificationStatus,
    pub allow_completion: bool,
    pub next_action: VerificationNextAction,
    pub summary: String,
    pub checks: serde_json::Value,
    pub evidence: Vec<Evidence>,
    pub failure_reasons: Vec<String>,
}

pub(super) struct VerifierV1;

impl VerifierV1 {
    pub(super) fn verify_completion(mut req: VerificationRequest) -> VerificationResult {
        let mut failure_reasons = Vec::new();
        let has_acceptance_criteria = req.acceptance_criteria.is_object()
            && !req
                .acceptance_criteria
                .as_object()
                .map(|o| o.is_empty())
                .unwrap_or(true);
        let all_planned_steps_executed = req.executed_steps == req.planned_steps;
        let contradiction = req.completion_claimed && req.failed_steps > 0;

        if !req.completion_claimed {
            failure_reasons.push("llm_did_not_claim_completion".to_string());
        }
        if req.failed_steps > 0 {
            failure_reasons.push("failed_steps_present".to_string());
        }
        if !all_planned_steps_executed {
            failure_reasons.push("not_all_planned_steps_executed".to_string());
        }
        if contradiction {
            failure_reasons.push("completion_claim_contradicted_by_failed_steps".to_string());
        }

        let high_risk = matches!(
            req.risk_class,
            GoalRiskClass::High | GoalRiskClass::Critical
        );

        let (status, allow_completion, next_action) = if !req.completion_claimed {
            (
                PlanVerificationStatus::Failed,
                false,
                VerificationNextAction::Replan,
            )
        } else if contradiction {
            let allow = !high_risk;
            (
                PlanVerificationStatus::Inconclusive,
                allow,
                if allow {
                    VerificationNextAction::Complete
                } else {
                    VerificationNextAction::Replan
                },
            )
        } else if high_risk && has_acceptance_criteria && req.failed_steps == 0 {
            // Soft gate for v1: require stronger evidence on high-impact work with explicit criteria.
            failure_reasons.push("high_risk_requires_more_evidence".to_string());
            (
                PlanVerificationStatus::Inconclusive,
                false,
                VerificationNextAction::GatherEvidence,
            )
        } else {
            (
                PlanVerificationStatus::Passed,
                true,
                VerificationNextAction::Complete,
            )
        };

        let checks = serde_json::json!({
            "completion_path": req.completion_path,
            "planned_steps": req.planned_steps,
            "executed_steps": req.executed_steps,
            "succeeded_steps": req.succeeded_steps,
            "failed_steps": req.failed_steps,
            "all_planned_steps_executed": all_planned_steps_executed,
            "completion_claimed": req.completion_claimed,
            "risk_class": req.risk_class,
            "has_acceptance_criteria": has_acceptance_criteria,
            "step_outcomes": req.step_outcomes,
            "failure_reasons": failure_reasons,
        });

        req.base_evidence.push(Evidence {
            kind: EvidenceKind::Observation,
            source: "verifier_v1".to_string(),
            summary: "Verifier v1 completion gate decision".to_string(),
            confidence: 0.8,
            payload: checks.clone(),
            created_at: Utc::now(),
        });
        req.base_evidence.push(Evidence {
            kind: EvidenceKind::Observation,
            source: "verifier_v1.completion_candidate".to_string(),
            summary: req.completion_candidate_summary.chars().take(240).collect(),
            confidence: 0.5,
            payload: serde_json::json!({
                "completion_path": req.completion_path,
                "completion_candidate_truncated": req.completion_candidate_summary.chars().count() > 240
            }),
            created_at: Utc::now(),
        });

        let summary = match status {
            PlanVerificationStatus::Passed => {
                "Verifier passed planned execution completion".to_string()
            }
            PlanVerificationStatus::Failed => {
                format!(
                    "Verifier rejected completion and requires replan: {}",
                    failure_reasons.join(", ")
                )
            }
            PlanVerificationStatus::Inconclusive => {
                if allow_completion {
                    format!(
                        "Verifier marked completion inconclusive but allowed completion (soft gate): {}",
                        failure_reasons.join(", ")
                    )
                } else {
                    format!(
                        "Verifier blocked completion pending additional work/evidence: {}",
                        failure_reasons.join(", ")
                    )
                }
            }
        };

        VerificationResult {
            status,
            allow_completion,
            next_action,
            summary,
            checks,
            evidence: req.base_evidence,
            failure_reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request() -> VerificationRequest {
        VerificationRequest {
            completion_path: "final_llm_completion_check",
            completion_claimed: true,
            completion_candidate_summary: "done".to_string(),
            planned_steps: 2,
            executed_steps: 2,
            succeeded_steps: 2,
            failed_steps: 0,
            step_outcomes: vec![],
            base_evidence: vec![],
            acceptance_criteria: serde_json::json!({}),
            risk_class: GoalRiskClass::Medium,
        }
    }

    #[test]
    fn test_verifier_passes_clean_medium_risk_completion() {
        let out = VerifierV1::verify_completion(base_request());
        assert_eq!(out.status, PlanVerificationStatus::Passed);
        assert!(out.allow_completion);
        assert_eq!(out.next_action, VerificationNextAction::Complete);
    }

    #[test]
    fn test_verifier_blocks_high_risk_with_explicit_criteria_pending_evidence() {
        let mut req = base_request();
        req.risk_class = GoalRiskClass::High;
        req.acceptance_criteria = serde_json::json!({"tests":"must pass"});
        let out = VerifierV1::verify_completion(req);
        assert_eq!(out.status, PlanVerificationStatus::Inconclusive);
        assert!(!out.allow_completion);
        assert_eq!(out.next_action, VerificationNextAction::GatherEvidence);
    }

    #[test]
    fn test_verifier_soft_allows_low_risk_contradiction() {
        let mut req = base_request();
        req.risk_class = GoalRiskClass::Low;
        req.failed_steps = 1;
        req.succeeded_steps = 1;
        let out = VerifierV1::verify_completion(req);
        assert_eq!(out.status, PlanVerificationStatus::Inconclusive);
        assert!(out.allow_completion);
    }

    #[test]
    fn test_verifier_blocks_high_risk_contradiction_and_requests_replan() {
        let mut req = base_request();
        req.risk_class = GoalRiskClass::Critical;
        req.failed_steps = 1;
        req.succeeded_steps = 1;
        let out = VerifierV1::verify_completion(req);
        assert_eq!(out.status, PlanVerificationStatus::Inconclusive);
        assert!(!out.allow_completion);
        assert_eq!(out.next_action, VerificationNextAction::Replan);
    }
}
