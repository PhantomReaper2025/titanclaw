//! Planner v1 runtime wrapper.
//!
//! Phase 1 keeps `Reasoning::plan()` as the planning generator and layers
//! validation + normalized plan trace formatting around it.

use crate::error::{Error, WorkerError};
use crate::llm::{ActionPlan, Reasoning, ReasoningContext};

#[derive(Debug, Clone)]
pub(super) struct PlannerOutput {
    pub action_plan: ActionPlan,
    pub plan_trace_summary: String,
}

pub(super) struct PlannerV1;

impl PlannerV1 {
    pub(super) async fn plan_initial(
        reasoning: &Reasoning,
        reason_ctx: &ReasoningContext,
    ) -> Result<PlannerOutput, Error> {
        let action_plan = reasoning.plan(reason_ctx).await?;
        validate_action_plan(&action_plan)?;
        let plan_trace_summary = render_plan_trace_summary(&action_plan);
        Ok(PlannerOutput {
            action_plan,
            plan_trace_summary,
        })
    }
}

fn validate_action_plan(plan: &ActionPlan) -> Result<(), Error> {
    if plan.goal.trim().is_empty() {
        return Err(WorkerError::ExecutionFailed {
            reason: "Planner returned an empty goal".to_string(),
        }
        .into());
    }
    if plan.actions.is_empty() {
        return Err(WorkerError::ExecutionFailed {
            reason: "Planner returned an empty action list".to_string(),
        }
        .into());
    }
    if !(0.0..=1.0).contains(&plan.confidence) {
        return Err(WorkerError::ExecutionFailed {
            reason: format!(
                "Planner returned invalid confidence {}; expected 0.0..=1.0",
                plan.confidence
            ),
        }
        .into());
    }
    Ok(())
}

pub(super) fn render_plan_trace_summary(plan: &ActionPlan) -> String {
    format!(
        "I've created a plan to accomplish this goal: {}\n\nSteps:\n{}",
        plan.goal,
        plan.actions
            .iter()
            .enumerate()
            .map(|(i, a)| format!("{}. {} - {}", i + 1, a.tool_name, a.reasoning))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ActionPlan;

    fn sample_plan() -> ActionPlan {
        serde_json::from_value(serde_json::json!({
            "goal": "Ship feature",
            "actions": [{
                "tool_name": "shell",
                "parameters": {"command":"cargo check"},
                "reasoning": "Validate compile",
                "expected_outcome": "Build passes"
            }],
            "estimated_cost": null,
            "estimated_time_secs": 30,
            "confidence": 0.8
        }))
        .expect("sample ActionPlan")
    }

    #[test]
    fn test_render_plan_trace_summary_formats_steps() {
        let s = render_plan_trace_summary(&sample_plan());
        assert!(s.contains("Ship feature"));
        assert!(s.contains("1. shell - Validate compile"));
    }

    #[test]
    fn test_validate_action_plan_rejects_empty_actions() {
        let mut plan = sample_plan();
        plan.actions.clear();
        let err = validate_action_plan(&plan).unwrap_err();
        assert!(err.to_string().contains("empty action list"));
    }

    #[test]
    fn test_validate_action_plan_rejects_invalid_confidence() {
        let mut plan = sample_plan();
        plan.confidence = 1.5;
        let err = validate_action_plan(&plan).unwrap_err();
        assert!(err.to_string().contains("invalid confidence"));
    }
}
