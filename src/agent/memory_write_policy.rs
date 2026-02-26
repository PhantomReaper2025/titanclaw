#![allow(dead_code)]

use serde_json::Value;

use crate::agent::{
    ConsolidationAction, MemoryRecordStatus, MemorySensitivity, MemorySourceKind, MemoryType,
};

#[derive(Debug, Clone)]
pub(super) struct MemoryWritePolicyConfig {
    pub working_ttl_secs: i64,
    pub episodic_ttl_secs: i64,
}

impl Default for MemoryWritePolicyConfig {
    fn default() -> Self {
        Self {
            working_ttl_secs: 24 * 60 * 60,
            episodic_ttl_secs: 30 * 24 * 60 * 60,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum MemoryWriteIntent {
    WorkerStepOutcome {
        success: bool,
    },
    VerifierOutcome {
        passed: bool,
        high_risk: bool,
        contradiction_detected: bool,
    },
    PolicyOutcome {
        denied: bool,
        requires_approval: bool,
    },
    ToolMemoryWrite {
        target: String,
    },
    RoutineSummary {
        successful: bool,
    },
    ConsolidatorOutput {
        action: ConsolidationAction,
    },
    Generic,
}

#[derive(Debug, Clone)]
pub(super) struct MemoryWriteCandidate {
    pub source_kind: MemorySourceKind,
    pub category: String,
    pub title: String,
    pub summary: String,
    pub payload: Value,
    pub provenance: Value,
    pub desired_memory_type: Option<MemoryType>,
    pub confidence_hint: Option<f32>,
    pub sensitivity_hint: Option<MemorySensitivity>,
    pub ttl_secs_hint: Option<i64>,
    pub high_impact: bool,
    pub intent: MemoryWriteIntent,
}

#[derive(Debug, Clone)]
pub(super) struct MemoryWriteDecision {
    pub allow: bool,
    pub memory_type: MemoryType,
    pub status: MemoryRecordStatus,
    pub reason_codes: Vec<String>,
    pub confidence: f32,
    pub sensitivity: MemorySensitivity,
    pub ttl_secs: Option<i64>,
    pub required_provenance_fields: Vec<String>,
}

pub(super) struct MemoryWritePolicyEngine {
    cfg: MemoryWritePolicyConfig,
}

impl MemoryWritePolicyEngine {
    pub(super) fn new(cfg: MemoryWritePolicyConfig) -> Self {
        Self { cfg }
    }

    pub(super) fn classify(&self, candidate: &MemoryWriteCandidate) -> MemoryWriteDecision {
        let mut decision = MemoryWriteDecision {
            allow: true,
            memory_type: candidate
                .desired_memory_type
                .unwrap_or(MemoryType::Episodic),
            status: MemoryRecordStatus::Active,
            reason_codes: vec!["classified".to_string()],
            confidence: candidate.confidence_hint.unwrap_or(0.7).clamp(0.0, 1.0),
            sensitivity: candidate
                .sensitivity_hint
                .unwrap_or(MemorySensitivity::Internal),
            ttl_secs: candidate.ttl_secs_hint,
            required_provenance_fields: vec![],
        };

        match &candidate.intent {
            MemoryWriteIntent::WorkerStepOutcome { success } => {
                decision.memory_type = MemoryType::Episodic;
                decision.confidence = if *success { 0.90 } else { 0.75 };
                decision.ttl_secs = Some(self.cfg.episodic_ttl_secs);
                decision.reason_codes.push(if *success {
                    "worker_step_success".to_string()
                } else {
                    "worker_step_failure".to_string()
                });
            }
            MemoryWriteIntent::VerifierOutcome {
                passed,
                high_risk,
                contradiction_detected,
            } => {
                decision.memory_type = MemoryType::Episodic;
                decision.confidence = if *passed { 0.9 } else { 0.85 };
                decision.ttl_secs = Some(self.cfg.episodic_ttl_secs);
                if *high_risk {
                    decision.reason_codes.push("verifier_high_risk".to_string());
                }
                if *contradiction_detected {
                    decision
                        .reason_codes
                        .push("verifier_contradiction".to_string());
                }
            }
            MemoryWriteIntent::PolicyOutcome {
                denied,
                requires_approval,
            } => {
                decision.memory_type = MemoryType::Episodic;
                decision.confidence = 0.95;
                decision.ttl_secs = None;
                if *denied {
                    decision.reason_codes.push("policy_denial".to_string());
                }
                if *requires_approval {
                    decision.reason_codes.push("approval_flow".to_string());
                }
            }
            MemoryWriteIntent::ToolMemoryWrite { target } => match target.as_str() {
                "daily_log" => {
                    decision.memory_type = MemoryType::Episodic;
                    decision.confidence = candidate.confidence_hint.unwrap_or(0.8);
                    decision.ttl_secs = Some(self.cfg.episodic_ttl_secs);
                    decision
                        .reason_codes
                        .push("tool_target_daily_log".to_string());
                }
                "heartbeat" => {
                    if looks_procedural_heartbeat(candidate) {
                        decision.memory_type = MemoryType::Procedural;
                        decision.ttl_secs = None;
                        decision
                            .reason_codes
                            .push("tool_target_heartbeat_procedural".to_string());
                    } else {
                        decision.memory_type = MemoryType::Working;
                        decision.ttl_secs = Some(self.cfg.working_ttl_secs);
                        decision
                            .reason_codes
                            .push("tool_target_heartbeat_working".to_string());
                    }
                    decision.confidence = candidate.confidence_hint.unwrap_or(0.75);
                }
                _ => {
                    decision.memory_type = candidate
                        .desired_memory_type
                        .unwrap_or(MemoryType::Semantic);
                    decision.confidence = candidate.confidence_hint.unwrap_or(0.65);
                    decision.reason_codes.push("tool_target_memory".to_string());
                }
            },
            MemoryWriteIntent::RoutineSummary { successful } => {
                decision.memory_type = MemoryType::Episodic;
                decision.confidence = if *successful { 0.88 } else { 0.76 };
                decision.ttl_secs = Some(self.cfg.episodic_ttl_secs);
                decision.reason_codes.push("routine_summary".to_string());
            }
            MemoryWriteIntent::ConsolidatorOutput { action } => match action {
                ConsolidationAction::PromoteSemantic => {
                    decision.memory_type = MemoryType::Semantic;
                    decision.ttl_secs = None;
                    decision.confidence = candidate.confidence_hint.unwrap_or(0.78);
                    decision
                        .reason_codes
                        .push("consolidator_promote_semantic".to_string());
                }
                ConsolidationAction::GeneratePlaybook => {
                    decision.memory_type = MemoryType::Procedural;
                    decision.ttl_secs = None;
                    decision.confidence = candidate.confidence_hint.unwrap_or(0.8);
                    decision
                        .reason_codes
                        .push("consolidator_generate_playbook".to_string());
                }
                ConsolidationAction::ArchiveSummary => {
                    decision.memory_type = MemoryType::Episodic;
                    decision.status = MemoryRecordStatus::Archived;
                    decision.ttl_secs = None;
                    decision.confidence = candidate.confidence_hint.unwrap_or(0.75);
                    decision
                        .reason_codes
                        .push("consolidator_archive_summary".to_string());
                }
                ConsolidationAction::Demote => {
                    decision.status = MemoryRecordStatus::Demoted;
                    decision
                        .reason_codes
                        .push("consolidator_demote".to_string());
                }
                ConsolidationAction::Reject => {
                    decision.allow = false;
                    decision.status = MemoryRecordStatus::Rejected;
                    decision
                        .reason_codes
                        .push("consolidator_reject".to_string());
                }
            },
            MemoryWriteIntent::Generic => {
                decision.reason_codes.push("generic_default".to_string());
            }
        }

        if matches!(
            decision.memory_type,
            MemoryType::Semantic | MemoryType::Procedural
        ) {
            decision.required_provenance_fields =
                vec!["source".to_string(), "timestamp".to_string()];
            if !has_required_provenance(&candidate.provenance) {
                decision
                    .reason_codes
                    .push("missing_provenance_fields".to_string());
                if candidate.confidence_hint.is_none() {
                    decision.confidence = decision.confidence.min(0.65);
                }
            }
        }

        if candidate.high_impact
            && matches!(
                decision.memory_type,
                MemoryType::Semantic | MemoryType::Procedural
            )
            && decision.confidence < 0.75
        {
            decision.allow = false;
            decision.status = MemoryRecordStatus::Rejected;
            decision
                .reason_codes
                .push("low_confidence_high_impact_rejected".to_string());
        }

        decision
    }
}

impl Default for MemoryWritePolicyEngine {
    fn default() -> Self {
        Self::new(MemoryWritePolicyConfig::default())
    }
}

fn has_required_provenance(provenance: &Value) -> bool {
    let Some(obj) = provenance.as_object() else {
        return false;
    };
    obj.get("source").is_some() && obj.get("timestamp").is_some()
}

fn looks_procedural_heartbeat(candidate: &MemoryWriteCandidate) -> bool {
    let haystack = format!(
        "{} {} {} {}",
        candidate.category, candidate.title, candidate.summary, candidate.payload
    )
    .to_lowercase();

    ["checklist", "runbook", "procedure", "playbook", "step"]
        .iter()
        .any(|token| haystack.contains(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_candidate(intent: MemoryWriteIntent) -> MemoryWriteCandidate {
        MemoryWriteCandidate {
            source_kind: MemorySourceKind::ToolMemoryWrite,
            category: "test".to_string(),
            title: "title".to_string(),
            summary: "summary".to_string(),
            payload: json!({"ok": true}),
            provenance: json!({}),
            desired_memory_type: None,
            confidence_hint: None,
            sensitivity_hint: None,
            ttl_secs_hint: None,
            high_impact: false,
            intent,
        }
    }

    #[test]
    fn classifies_worker_step_success_as_episodic() {
        let engine = MemoryWritePolicyEngine::default();
        let candidate = base_candidate(MemoryWriteIntent::WorkerStepOutcome { success: true });
        let decision = engine.classify(&candidate);

        assert!(decision.allow);
        assert_eq!(decision.memory_type, MemoryType::Episodic);
        assert_eq!(decision.status, MemoryRecordStatus::Active);
        assert_eq!(decision.confidence, 0.90);
        assert_eq!(decision.ttl_secs, Some(30 * 24 * 60 * 60));
    }

    #[test]
    fn maps_daily_log_tool_writes_to_episodic_with_ttl() {
        let engine = MemoryWritePolicyEngine::default();
        let mut candidate = base_candidate(MemoryWriteIntent::ToolMemoryWrite {
            target: "daily_log".to_string(),
        });
        candidate.source_kind = MemorySourceKind::ToolMemoryWrite;
        let decision = engine.classify(&candidate);

        assert!(decision.allow);
        assert_eq!(decision.memory_type, MemoryType::Episodic);
        assert_eq!(decision.confidence, 0.8);
        assert_eq!(decision.ttl_secs, Some(30 * 24 * 60 * 60));
    }

    #[test]
    fn classifies_heartbeat_runbook_as_procedural() {
        let engine = MemoryWritePolicyEngine::default();
        let mut candidate = base_candidate(MemoryWriteIntent::ToolMemoryWrite {
            target: "heartbeat".to_string(),
        });
        candidate.summary = "Runbook checklist for service restart".to_string();
        let decision = engine.classify(&candidate);

        assert!(decision.allow);
        assert_eq!(decision.memory_type, MemoryType::Procedural);
        assert_eq!(decision.ttl_secs, None);
    }

    #[test]
    fn rejects_low_confidence_high_impact_semantic_candidate() {
        let engine = MemoryWritePolicyEngine::default();
        let mut candidate = base_candidate(MemoryWriteIntent::ToolMemoryWrite {
            target: "memory".to_string(),
        });
        candidate.high_impact = true;
        candidate.desired_memory_type = Some(MemoryType::Semantic);
        candidate.confidence_hint = Some(0.4);
        let decision = engine.classify(&candidate);

        assert!(!decision.allow);
        assert_eq!(decision.status, MemoryRecordStatus::Rejected);
        assert!(
            decision
                .reason_codes
                .iter()
                .any(|r| r == "low_confidence_high_impact_rejected")
        );
    }

    #[test]
    fn semantic_candidates_require_provenance_fields() {
        let engine = MemoryWritePolicyEngine::default();
        let mut candidate = base_candidate(MemoryWriteIntent::ToolMemoryWrite {
            target: "memory".to_string(),
        });
        candidate.desired_memory_type = Some(MemoryType::Semantic);
        let decision = engine.classify(&candidate);

        assert_eq!(decision.memory_type, MemoryType::Semantic);
        assert_eq!(
            decision.required_provenance_fields,
            vec!["source".to_string(), "timestamp".to_string()]
        );
        assert!(
            decision
                .reason_codes
                .iter()
                .any(|r| r == "missing_provenance_fields")
        );
    }
}
