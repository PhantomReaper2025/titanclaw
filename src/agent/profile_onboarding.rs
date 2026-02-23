use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::Agent;
use crate::agent::submission::Submission;
use crate::channels::IncomingMessage;
use crate::db::Database;
use crate::error::Error;
use crate::workspace::{Workspace, paths};

const VERSION: u32 = 1;
const KEY_COMPLETED: &str = "profile_onboard.completed";
const KEY_VERSION: &str = "profile_onboard.version";
const KEY_STATE: &str = "profile_onboard.state";
const KEY_DEFERRED_UNTIL: &str = "profile_onboard.deferred_until";
const KEY_LAST_COMPLETED_AT: &str = "profile_onboard.last_completed_at";

const MANAGED_BEGIN_PREFIX: &str = "<!-- titanclaw:managed-begin profile-onboarding:v1";
const MANAGED_END_MARKER: &str = "<!-- titanclaw:managed-end profile-onboarding:v1 -->";

#[derive(Clone)]
pub struct ProfileOnboardingManager {
    store: Arc<dyn Database>,
    workspace: Arc<Workspace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileOnboardingState {
    pub version: u32,
    pub mode: OnboardingMode,
    pub queue: Vec<String>,
    pub current_index: usize,
    pub answers: BTreeMap<String, String>,
    pub followups_used: usize,
    pub pending_original_request: Option<String>,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OnboardingMode {
    Asking,
    Reviewing,
}

#[derive(Debug, Clone)]
pub enum OnboardingIntercept {
    None,
    Respond(String),
}

impl ProfileOnboardingManager {
    pub fn new(store: Arc<dyn Database>, workspace: Arc<Workspace>) -> Self {
        Self { store, workspace }
    }

    pub async fn is_completed(&self, user_id: &str) -> bool {
        self.store
            .get_setting(user_id, KEY_COMPLETED)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub async fn is_deferred(&self, user_id: &str) -> bool {
        let Some(value) = self
            .store
            .get_setting(user_id, KEY_DEFERRED_UNTIL)
            .await
            .ok()
            .flatten()
        else {
            return false;
        };
        let Some(raw) = value.as_str() else {
            return false;
        };
        chrono::DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|dt| dt.with_timezone(&Utc) > Utc::now())
            .unwrap_or(false)
    }

    pub async fn clear_defer(&self, user_id: &str) {
        let _ = self.store.delete_setting(user_id, KEY_DEFERRED_UNTIL).await;
    }

    pub async fn get_state(&self, user_id: &str) -> Option<ProfileOnboardingState> {
        self.store
            .get_setting(user_id, KEY_STATE)
            .await
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value::<ProfileOnboardingState>(v).ok())
    }

    async fn save_state(
        &self,
        user_id: &str,
        state: &ProfileOnboardingState,
    ) -> Result<(), String> {
        let value = serde_json::to_value(state).map_err(|e| e.to_string())?;
        self.store
            .set_setting(user_id, KEY_STATE, &value)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn clear_state(&self, user_id: &str) {
        let _ = self.store.delete_setting(user_id, KEY_STATE).await;
    }

    pub async fn status_text(&self, user_id: &str) -> String {
        let completed = self.is_completed(user_id).await;
        let deferred = self.is_deferred(user_id).await;
        let in_progress = self.get_state(user_id).await.is_some();
        let last_completed = self
            .store
            .get_setting(user_id, KEY_LAST_COMPLETED_AT)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(str::to_string));
        format!(
            "Profile onboarding status:\n- completed: {}\n- in_progress: {}\n- deferred: {}\n- version: {}\n- last_completed_at: {}",
            completed,
            in_progress,
            deferred,
            VERSION,
            last_completed.unwrap_or_else(|| "never".to_string())
        )
    }

    pub async fn start(
        &self,
        user_id: &str,
        pending_original_request: Option<&str>,
    ) -> Result<String, String> {
        let mut state = ProfileOnboardingState {
            version: VERSION,
            mode: OnboardingMode::Asking,
            queue: core_question_ids(),
            current_index: 0,
            answers: BTreeMap::new(),
            followups_used: 0,
            pending_original_request: pending_original_request.map(|s| s.to_string()),
            started_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        state.updated_at = Utc::now().to_rfc3339();
        self.save_state(user_id, &state).await?;
        self.clear_defer(user_id).await;
        Ok(format!(
            "{}\n\n{}",
            intro_text(),
            self.render_current_question(&state)
        ))
    }

    pub async fn defer(&self, user_id: &str) -> Result<String, String> {
        let until = (Utc::now() + ChronoDuration::hours(24)).to_rfc3339();
        self.store
            .set_setting(
                user_id,
                KEY_DEFERRED_UNTIL,
                &serde_json::Value::String(until.clone()),
            )
            .await
            .map_err(|e| e.to_string())?;
        self.clear_state(user_id).await;
        Ok(format!(
            "Profile onboarding deferred until {}.\nUse `/onboard profile` anytime to continue.",
            until
        ))
    }

    pub async fn skip(&self, user_id: &str) -> Result<String, String> {
        self.defer(user_id).await
    }

    pub async fn reset(&self, user_id: &str) -> Result<String, String> {
        self.clear_state(user_id).await;
        let _ = self.store.delete_setting(user_id, KEY_COMPLETED).await;
        let _ = self.store.delete_setting(user_id, KEY_VERSION).await;
        let _ = self
            .store
            .delete_setting(user_id, KEY_LAST_COMPLETED_AT)
            .await;
        Ok("Profile onboarding reset. Use `/onboard profile` to start again.".to_string())
    }

    pub async fn handle_answer(&self, user_id: &str, input: &str) -> Result<String, String> {
        let mut state = self.get_state(user_id).await.ok_or_else(|| {
            "No active profile onboarding session. Use `/onboard profile`.".to_string()
        })?;

        let trimmed = input.trim();
        let lower = trimmed.to_lowercase();

        if state.mode == OnboardingMode::Reviewing {
            return self
                .handle_review_input(user_id, state, trimmed, &lower)
                .await;
        }

        if lower == "defer" {
            return self.defer(user_id).await;
        }
        if lower == "skip" {
            return self.skip(user_id).await;
        }

        let Some(qid) = state.queue.get(state.current_index).cloned() else {
            state.mode = OnboardingMode::Reviewing;
            state.updated_at = Utc::now().to_rfc3339();
            self.save_state(user_id, &state).await?;
            return Ok(self.render_review(&state));
        };

        state.answers.insert(qid.clone(), trimmed.to_string());
        self.maybe_add_followups(&mut state, &qid, trimmed);
        state.current_index = state.current_index.saturating_add(1);
        state.updated_at = Utc::now().to_rfc3339();

        if state.current_index >= state.queue.len() {
            state.mode = OnboardingMode::Reviewing;
            self.save_state(user_id, &state).await?;
            return Ok(self.render_review(&state));
        }

        self.save_state(user_id, &state).await?;
        Ok(self.render_current_question(&state))
    }

    async fn handle_review_input(
        &self,
        user_id: &str,
        mut state: ProfileOnboardingState,
        raw: &str,
        lower: &str,
    ) -> Result<String, String> {
        match lower {
            "confirm" | "save" | "yes" => {
                self.apply_to_workspace(user_id, &state).await?;
                self.store
                    .set_setting(user_id, KEY_COMPLETED, &serde_json::Value::Bool(true))
                    .await
                    .map_err(|e| e.to_string())?;
                self.store
                    .set_setting(
                        user_id,
                        KEY_VERSION,
                        &serde_json::Value::Number(serde_json::Number::from(VERSION)),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                self.store
                    .set_setting(
                        user_id,
                        KEY_LAST_COMPLETED_AT,
                        &serde_json::Value::String(Utc::now().to_rfc3339()),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                self.clear_defer(user_id).await;
                self.clear_state(user_id).await;

                let mut out = String::from(
                    "Profile onboarding saved to managed sections in AGENTS.md, IDENTITY.md, SOUL.md, USER.md, and MEMORY.md.",
                );
                if let Some(pending) = state.pending_original_request.take() {
                    out.push_str("\n\nYour earlier request was paused during onboarding:\n");
                    out.push_str(&format!("> {}", truncate_line(&pending, 220)));
                    out.push_str("\n\nSend it again (or say `continue that request`) to proceed.");
                }
                Ok(out)
            }
            "defer" => self.defer(user_id).await,
            "skip" => self.skip(user_id).await,
            _ if lower.starts_with("edit ") => {
                let topic = raw.trim()[5..].trim();
                if let Some(idx) = find_question_index(&state.queue, topic) {
                    state.mode = OnboardingMode::Asking;
                    state.current_index = idx;
                    state.updated_at = Utc::now().to_rfc3339();
                    self.save_state(user_id, &state).await?;
                    Ok(format!(
                        "Editing `{}`.\n\n{}",
                        state.queue[idx],
                        self.render_current_question(&state)
                    ))
                } else {
                    Ok("Unknown topic to edit. Use `edit <question_id>` or `confirm`.".to_string())
                }
            }
            _ => Ok(
                "Review mode: reply with `confirm`, `defer`, `skip`, or `edit <question_id>`."
                    .to_string(),
            ),
        }
    }

    fn maybe_add_followups(&self, state: &mut ProfileOnboardingState, qid: &str, answer: &str) {
        if state.followups_used >= 3 {
            return;
        }
        let mut existing: HashSet<String> = state.queue.iter().cloned().collect();
        let lower = answer.to_lowercase();
        let mut maybe_push = |id: &str| {
            if state.followups_used >= 3 || existing.contains(id) {
                return;
            }
            let insert_at = state.current_index + 1;
            if insert_at <= state.queue.len() {
                state.queue.insert(insert_at, id.to_string());
                existing.insert(id.to_string());
                state.followups_used += 1;
            }
        };

        match qid {
            "primary_goals" => {
                let words = lower.split_whitespace().count();
                if words <= 3 || lower == "coding" || lower == "everything" {
                    maybe_push("followup_primary_goals_stack");
                }
            }
            "communication_tone" => {
                if lower.contains("direct") && !lower.contains("detail") {
                    maybe_push("followup_tone_detail");
                }
            }
            "execution_style" => {
                if lower.contains("proactive")
                    || lower.contains("don't ask")
                    || lower.contains("dont ask")
                {
                    maybe_push("followup_execution_confirmation");
                }
            }
            _ => {}
        }
    }

    fn render_current_question(&self, state: &ProfileOnboardingState) -> String {
        let total = state.queue.len();
        let idx = state.current_index + 1;
        let qid = state
            .queue
            .get(state.current_index)
            .map(String::as_str)
            .unwrap_or("done");
        format!(
            "Profile Onboarding {}/{}\n\n{}\n\n(Reply with your answer. You can also type `defer` or `skip`.)",
            idx,
            total,
            question_prompt(qid)
        )
    }

    fn render_review(&self, state: &ProfileOnboardingState) -> String {
        let summary = render_review_summary(state);
        format!(
            "{}\n\nReply `confirm` to save, `defer` to continue later, `skip` to dismiss for now, or `edit <question_id>`.",
            summary
        )
    }

    async fn apply_to_workspace(
        &self,
        user_id: &str,
        state: &ProfileOnboardingState,
    ) -> Result<(), String> {
        let docs = build_doc_sections(user_id, state);
        for (path, body) in docs {
            let existing = self
                .workspace
                .read(path)
                .await
                .map(|d| d.content)
                .unwrap_or_default();
            let updated = render_with_managed_section(&existing, path, &body)?;
            self.workspace
                .write(path, &updated)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Agent {
    pub(super) async fn maybe_handle_profile_onboarding(
        &self,
        message: &IncomingMessage,
        submission: &Submission,
    ) -> Result<Option<String>, Error> {
        let Some(mgr) = self.profile_onboarding() else {
            return Ok(None);
        };

        match submission {
            Submission::SystemCommand { command, args } if command == "onboard" => {
                return Ok(Some(
                    self.handle_profile_onboarding_command(mgr, &message.user_id, args)
                        .await,
                ));
            }
            _ => {}
        }

        let Submission::UserInput { content } = submission else {
            return Ok(None);
        };

        // Allow normal chat if completed or deferred and no active onboarding session.
        let active = mgr.get_state(&message.user_id).await.is_some();
        let should_offer = if active {
            true
        } else if mgr.is_completed(&message.user_id).await {
            false
        } else {
            !mgr.is_deferred(&message.user_id).await
        };

        if !should_offer {
            return Ok(None);
        }

        let response = if active {
            mgr.handle_answer(&message.user_id, content)
                .await
                .unwrap_or_else(|e| format!("Profile onboarding error: {}", e))
        } else {
            mgr.start(&message.user_id, Some(content))
                .await
                .unwrap_or_else(|e| format!("Failed to start profile onboarding: {}", e))
        };
        Ok(Some(response))
    }

    async fn handle_profile_onboarding_command(
        &self,
        mgr: &ProfileOnboardingManager,
        user_id: &str,
        args: &[String],
    ) -> String {
        let sub = args.first().map(|s| s.to_ascii_lowercase());
        let rest = args.get(1).map(|s| s.to_ascii_lowercase());
        match (sub.as_deref(), rest.as_deref()) {
            (None, _) => "Usage: /onboard profile [status|defer|skip|reset]\nAlso works in chat automatically on first use.".to_string(),
            (Some("profile"), None) => mgr
                .start(user_id, None)
                .await
                .unwrap_or_else(|e| format!("Failed to start profile onboarding: {}", e)),
            (Some("profile"), Some("status")) => mgr.status_text(user_id).await,
            (Some("profile"), Some("defer")) => mgr
                .defer(user_id)
                .await
                .unwrap_or_else(|e| format!("Failed to defer profile onboarding: {}", e)),
            (Some("profile"), Some("skip")) => mgr
                .skip(user_id)
                .await
                .unwrap_or_else(|e| format!("Failed to skip profile onboarding: {}", e)),
            (Some("profile"), Some("reset")) => mgr
                .reset(user_id)
                .await
                .unwrap_or_else(|e| format!("Failed to reset profile onboarding: {}", e)),
            _ => "Usage: /onboard profile [status|defer|skip|reset]".to_string(),
        }
    }
}

fn intro_text() -> String {
    "Before we start, I want to personalize TitanClaw the way OpenClaw-style onboarding does: who you are, what I should mainly do, and how I should speak/work with you.\nThis takes ~1-2 minutes and is saved into managed memory docs. You can type `defer` to do it later.".to_string()
}

fn core_question_ids() -> Vec<String> {
    vec![
        "assistant_name_identity",
        "user_identity",
        "primary_goals",
        "current_projects_context",
        "communication_tone",
        "execution_style",
        "boundaries_constraints",
        "tooling_preferences",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn question_prompt(id: &str) -> &'static str {
    match id {
        "assistant_name_identity" => {
            "What should I call myself (assistant name/persona), if anything?"
        }
        "user_identity" => {
            "Who are you / what should I call you? (Name, nickname, pronouns optional)"
        }
        "primary_goals" => "What should I mainly help you with most of the time?",
        "current_projects_context" => {
            "What projects, domains, or context should I know about right now?"
        }
        "communication_tone" => "How should I speak to you? (tone, directness, detail level)",
        "execution_style" => {
            "How proactive should I be? (ask less vs ask more, defaults vs confirmations)"
        }
        "boundaries_constraints" => {
            "Any hard rules, safety boundaries, or things I should never do?"
        }
        "tooling_preferences" => "Preferred tools/models/runtimes/workflows I should default to?",
        "followup_primary_goals_stack" => {
            "What stack/languages/frameworks should I prioritize for that work?"
        }
        "followup_tone_detail" => {
            "When you say direct, do you also want concise answers by default, or detailed technical breakdowns?"
        }
        "followup_execution_confirmation" => {
            "When should I still ask before acting? (e.g. destructive commands, public posting, spending money, external messages)"
        }
        _ => "Tell me anything important I should know.",
    }
}

fn render_review_summary(state: &ProfileOnboardingState) -> String {
    let mut out = String::from("Profile Onboarding Review\n\n");
    let sections = [
        ("assistant_name_identity", "Assistant identity"),
        ("user_identity", "User identity"),
        ("primary_goals", "Primary goals"),
        ("current_projects_context", "Current projects/context"),
        ("communication_tone", "Communication style"),
        ("execution_style", "Execution style"),
        ("boundaries_constraints", "Boundaries"),
        ("tooling_preferences", "Tooling preferences"),
        ("followup_primary_goals_stack", "Stack/languages"),
        ("followup_tone_detail", "Detail level"),
        ("followup_execution_confirmation", "Confirmation rules"),
    ];
    for (qid, label) in sections {
        if let Some(v) = state.answers.get(qid) {
            out.push_str(&format!("- {}: {}\n", label, v));
        }
    }
    out
}

fn build_doc_sections(
    user_id: &str,
    state: &ProfileOnboardingState,
) -> [(&'static str, String); 5] {
    let a = |k: &str| state.answers.get(k).map(|s| s.trim()).unwrap_or("");
    let ts = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");

    let user_doc = format!(
        "## Profile Onboarding (Managed)\n_Updated: {}_\n\n- User ID: `{}`\n- What to call them: {}\n- Main goals: {}\n- Current context/projects: {}\n- Communication preferences: {}\n- Execution preferences: {}\n- Tooling/runtime preferences: {}\n",
        ts,
        user_id,
        md(a("user_identity")),
        md(a("primary_goals")),
        md(a("current_projects_context")),
        md(a("communication_tone")),
        md(a("execution_style")),
        md(a("tooling_preferences")),
    );

    let identity_doc = format!(
        "## Profile Onboarding (Managed)\n_Updated: {}_\n\n- Assistant identity/persona: {}\n- Preferred interaction tone: {}\n",
        ts,
        md(a("assistant_name_identity")),
        md(a("communication_tone")),
    );

    let agents_doc = format!(
        "## Profile Onboarding (Managed)\n_Updated: {}_\n\n- Explicit execution preference: {}\n- Explicit operational constraints: {}\n- Preferred tooling defaults: {}\n",
        ts,
        md(a("execution_style")),
        md(a("boundaries_constraints")),
        md(a("tooling_preferences")),
    );

    let soul_doc = format!(
        "## Profile Onboarding (Managed)\n_Updated: {}_\n\n- Explicit boundaries / values from onboarding: {}\n",
        ts,
        md(a("boundaries_constraints")),
    );

    let memory_doc = format!(
        "## Profile Onboarding Snapshot (Managed)\n_Updated: {}_\n\n- Current priorities: {}\n- Context/projects: {}\n- Communication style: {}\n- Tooling defaults: {}\n",
        ts,
        md(a("primary_goals")),
        md(a("current_projects_context")),
        md(a("communication_tone")),
        md(a("tooling_preferences")),
    );

    [
        (paths::AGENTS, agents_doc),
        (paths::IDENTITY, identity_doc),
        (paths::SOUL, soul_doc),
        (paths::USER, user_doc),
        (paths::MEMORY, memory_doc),
    ]
}

fn md(s: &str) -> String {
    if s.is_empty() {
        "(not provided)".to_string()
    } else {
        s.to_string()
    }
}

fn managed_begin_marker(path: &str) -> String {
    format!("{MANAGED_BEGIN_PREFIX} file={path} source=profile-onboarding -->")
}

fn render_with_managed_section(
    content: &str,
    path: &str,
    managed_body: &str,
) -> Result<String, String> {
    let begin = managed_begin_marker(path);
    let block = format!("{}\n{}\n{}", begin, managed_body.trim(), MANAGED_END_MARKER);

    let starts: Vec<_> = content.match_indices(&begin).collect();
    if starts.len() > 1 {
        return Err(format!(
            "Multiple onboarding managed section starts found in {}",
            path
        ));
    }
    let ends: Vec<_> = content.match_indices(MANAGED_END_MARKER).collect();
    if let Some((start_idx, _)) = starts.first().copied() {
        let after_begin = start_idx + begin.len();
        let Some(end_rel) = content[after_begin..].find(MANAGED_END_MARKER) else {
            return Err(format!("Managed onboarding end marker missing in {}", path));
        };
        let end_idx = after_begin + end_rel + MANAGED_END_MARKER.len();
        let mut out = String::new();
        out.push_str(&content[..start_idx]);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&block);
        out.push_str(&content[end_idx..]);
        return Ok(out);
    }
    if ends.len() > 1 {
        return Err(format!(
            "Multiple onboarding managed section ends found in {}",
            path
        ));
    }
    if content.trim().is_empty() {
        Ok(block)
    } else {
        Ok(format!("{}\n\n{}", content.trim_end(), block))
    }
}

fn find_question_index(queue: &[String], topic: &str) -> Option<usize> {
    let t = topic.trim().to_lowercase();
    queue
        .iter()
        .position(|q| q.eq_ignore_ascii_case(&t) || q.replace('_', " ").eq_ignore_ascii_case(&t))
}

fn truncate_line(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        chars[..max].iter().collect::<String>() + "..."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_with_managed_section_insert_replace() {
        let initial = "# Title\n\nManual";
        let path = paths::USER;
        let a = render_with_managed_section(initial, path, "A").unwrap();
        assert!(a.contains("Manual"));
        assert!(a.contains("A"));
        let b = render_with_managed_section(&a, path, "B").unwrap();
        assert!(b.contains("Manual"));
        assert!(b.contains("B"));
        assert!(!b.contains("\nA\n"));
    }
}
