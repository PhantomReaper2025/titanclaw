#![allow(dead_code)]

//! Replanner v1 scaffolding (Phase 1 runtime integration lands incrementally).
//!
//! This module defines the runtime replan intent/reason types so worker and
//! verifier paths can standardize trigger semantics before full automatic
//! replan execution is wired in.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplanReason {
    PlanExhaustedIncomplete,
    StepFailure,
    PolicyDenied,
    VerifierInsufficientEvidence,
    BudgetExceeded,
    UserInterruption,
    EnvironmentDrift,
    ToolUnavailable,
}

impl ReplanReason {
    pub(super) fn code(self) -> &'static str {
        match self {
            ReplanReason::PlanExhaustedIncomplete => "plan_exhausted_incomplete",
            ReplanReason::StepFailure => "step_failure",
            ReplanReason::PolicyDenied => "policy_denied",
            ReplanReason::VerifierInsufficientEvidence => "verifier_insufficient_evidence",
            ReplanReason::BudgetExceeded => "budget_exceeded",
            ReplanReason::UserInterruption => "user_interruption",
            ReplanReason::EnvironmentDrift => "environment_drift",
            ReplanReason::ToolUnavailable => "tool_unavailable",
        }
    }
}

pub(super) struct ReplannerV1;

impl ReplannerV1 {
    pub(super) fn replan_prompt(reason: ReplanReason, detail: &str) -> String {
        format!(
            "The previous plan needs revision. Replan now.\nReason: {}.\nDetail: {}\nProduce a revised multi-step plan that avoids the failure mode and completes the job.",
            reason.code(),
            detail
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReplanBudgets {
    pub max_replans_per_job: u32,
}

impl Default for ReplanBudgets {
    fn default() -> Self {
        Self {
            max_replans_per_job: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct ReplanRuntimeState {
    pub replans_attempted: u32,
}

impl ReplanRuntimeState {
    pub(super) fn can_replan(&self, budgets: ReplanBudgets) -> bool {
        self.replans_attempted < budgets.max_replans_per_job
    }

    pub(super) fn record_replan(&mut self) {
        self.replans_attempted = self.replans_attempted.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replan_runtime_state_budget() {
        let budgets = ReplanBudgets {
            max_replans_per_job: 1,
        };
        let mut state = ReplanRuntimeState::default();
        assert!(state.can_replan(budgets));
        state.record_replan();
        assert!(!state.can_replan(budgets));
    }

    #[test]
    fn test_replan_prompt_includes_reason_code() {
        let prompt = ReplannerV1::replan_prompt(ReplanReason::PolicyDenied, "approval required");
        assert!(prompt.contains("policy_denied"));
        assert!(prompt.contains("approval required"));
    }
}
