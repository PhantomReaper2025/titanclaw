//! Background synthesis of operator/profile context into managed workspace docs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::llm::{ChatMessage, CompletionRequest, LlmProvider};
use crate::workspace::{Workspace, paths};

const MANAGED_BEGIN_PREFIX: &str = "<!-- titanclaw:managed-begin profile-synthesis:v1";
const MANAGED_END_MARKER: &str = "<!-- titanclaw:managed-end profile-synthesis:v1 -->";
const OBS_PATH: &str = "context/profile_observations.jsonl";
const STATE_PATH: &str = "context/profile_state.json";

#[derive(Debug, Clone)]
pub struct ProfileSynthesisConfig {
    pub enabled: bool,
    pub debounce: Duration,
    pub max_batch_turns: usize,
    pub min_input_chars: usize,
    pub llm_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileSynthesizer {
    tx: mpsc::Sender<SynthesisEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SynthesisEvent {
    user_id: String,
    thread_id: String,
    user_input: String,
    assistant_response: String,
    timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Observation {
    id: String,
    timestamp: String,
    thread_id: String,
    category: String,
    target_doc: String,
    statement: String,
    confidence: f32,
    explicit: bool,
    evidence_excerpt: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfileState {
    statements: HashMap<String, StatementRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StatementRecord {
    key: String,
    target_doc: String,
    category: String,
    statement: String,
    count: u32,
    confidence: f32,
    explicit: bool,
    last_seen: String,
}

impl ProfileSynthesizer {
    pub fn new(
        config: ProfileSynthesisConfig,
        workspace: Arc<Workspace>,
        llm: Arc<dyn LlmProvider>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(128);
        let this = Arc::new(Self { tx });
        tokio::spawn(run_worker(config, workspace, llm, rx));
        this
    }

    pub fn enqueue_turn(
        &self,
        user_id: &str,
        thread_id: &str,
        user_input: &str,
        assistant_response: &str,
    ) {
        let evt = SynthesisEvent {
            user_id: user_id.to_string(),
            thread_id: thread_id.to_string(),
            user_input: user_input.to_string(),
            assistant_response: assistant_response.to_string(),
            timestamp: Utc::now().to_rfc3339(),
        };
        if let Err(e) = self.tx.try_send(evt) {
            tracing::debug!("Profile synthesis queue full/dropped event: {}", e);
        }
    }
}

async fn run_worker(
    config: ProfileSynthesisConfig,
    workspace: Arc<Workspace>,
    llm: Arc<dyn LlmProvider>,
    mut rx: mpsc::Receiver<SynthesisEvent>,
) {
    if !config.enabled {
        return;
    }

    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < config.max_batch_turns {
            match tokio::time::timeout(config.debounce, rx.recv()).await {
                Ok(Some(next)) => batch.push(next),
                Ok(None) | Err(_) => break,
            }
        }

        if let Err(e) = process_batch(&config, &workspace, llm.as_ref(), &batch).await {
            tracing::warn!("Profile synthesis batch failed: {}", e);
        }
    }
}

async fn process_batch(
    config: &ProfileSynthesisConfig,
    workspace: &Workspace,
    llm: &dyn LlmProvider,
    batch: &[SynthesisEvent],
) -> Result<(), String> {
    let mut observations = Vec::new();
    for evt in batch {
        if evt.user_input.chars().count() < config.min_input_chars {
            continue;
        }
        observations.extend(extract_observations(evt));
    }
    if observations.is_empty() {
        return Ok(());
    }

    append_observations(workspace, &observations).await?;
    let mut state = load_state(workspace).await.unwrap_or_default();
    apply_observations_to_state(&mut state, &observations);

    let mut sections = build_deterministic_sections(&state);
    if config.llm_enabled {
        if let Ok(llm_sections) = llm_merge_sections(workspace, llm, &observations, &sections).await
        {
            for (file, content) in llm_sections {
                if !content.trim().is_empty() {
                    sections.insert(file, content);
                }
            }
        }
    }

    for file in [
        paths::AGENTS,
        paths::IDENTITY,
        paths::SOUL,
        paths::USER,
        paths::MEMORY,
    ] {
        let managed = sections
            .get(file)
            .cloned()
            .unwrap_or_else(|| "_No synthesized updates yet._".to_string());
        update_managed_section(workspace, file, &managed).await?;
    }

    save_state(workspace, &state).await?;
    Ok(())
}

fn normalize_statement_key(doc: &str, statement: &str) -> String {
    format!(
        "{}::{}",
        doc,
        statement
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn extract_observations(evt: &SynthesisEvent) -> Vec<Observation> {
    let mut out = Vec::new();
    let user = evt.user_input.trim();
    let lower = user.to_lowercase();
    let excerpt: String = user.chars().take(240).collect();

    let mut push =
        |target_doc: &str, category: &str, statement: String, confidence: f32, explicit: bool| {
            out.push(Observation {
                id: format!(
                    "{}:{}:{}",
                    evt.timestamp,
                    target_doc,
                    statement.chars().take(24).collect::<String>()
                ),
                timestamp: evt.timestamp.clone(),
                thread_id: evt.thread_id.clone(),
                category: category.to_string(),
                target_doc: target_doc.to_string(),
                statement,
                confidence,
                explicit,
                evidence_excerpt: excerpt.clone(),
            });
        };

    if lower.contains("prefer ") || lower.contains("i prefer") || lower.contains("i like") {
        push(paths::USER, "user_pref", user.to_string(), 0.95, true);
    }
    if lower.contains("i want") || lower.contains("i need") || lower.contains("please") {
        push(paths::USER, "user_pref", user.to_string(), 0.9, true);
        push(paths::MEMORY, "product_goal", user.to_string(), 0.75, true);
    }
    if lower.contains("don't ") || lower.contains("dont ") || lower.contains("do not ") {
        push(paths::AGENTS, "operator_rule", user.to_string(), 0.96, true);
    }
    if lower.starts_with("keep ") || lower.starts_with("always ") || lower.starts_with("never ") {
        push(paths::AGENTS, "operator_rule", user.to_string(), 0.93, true);
    }
    if lower.contains("security first") || lower.contains("reliability") || lower.contains("truth")
    {
        push(paths::SOUL, "core_value", user.to_string(), 0.8, true);
    }
    if lower.contains("titanclaw") || lower.contains("product name") || lower.contains("sandbox") {
        push(
            paths::IDENTITY,
            "identity_fact",
            user.to_string(),
            0.78,
            true,
        );
        push(paths::MEMORY, "durable_fact", user.to_string(), 0.7, true);
    }

    // Pull durable configuration mentions from assistant confirmations too.
    let resp_lower = evt.assistant_response.to_lowercase();
    if resp_lower.contains("fixed")
        || resp_lower.contains("implemented")
        || resp_lower.contains("supported")
    {
        let resp_excerpt: String = evt.assistant_response.chars().take(220).collect();
        out.push(Observation {
            id: format!("{}:assistant:{}", evt.timestamp, evt.thread_id),
            timestamp: evt.timestamp.clone(),
            thread_id: evt.thread_id.clone(),
            category: "assistant_summary".to_string(),
            target_doc: paths::MEMORY.to_string(),
            statement: resp_excerpt.clone(),
            confidence: 0.55,
            explicit: false,
            evidence_excerpt: resp_excerpt,
        });
    }

    out
}

fn apply_observations_to_state(state: &mut ProfileState, observations: &[Observation]) {
    for obs in observations {
        let key = normalize_statement_key(&obs.target_doc, &obs.statement);
        let entry = state
            .statements
            .entry(key.clone())
            .or_insert(StatementRecord {
                key: key.clone(),
                target_doc: obs.target_doc.clone(),
                category: obs.category.clone(),
                statement: obs.statement.clone(),
                count: 0,
                confidence: obs.confidence,
                explicit: obs.explicit,
                last_seen: obs.timestamp.clone(),
            });
        entry.count = entry.count.saturating_add(1);
        entry.confidence = entry.confidence.max(obs.confidence);
        entry.explicit = entry.explicit || obs.explicit;
        entry.last_seen = obs.timestamp.clone();
    }
}

fn should_include(record: &StatementRecord) -> bool {
    match record.target_doc.as_str() {
        paths::AGENTS => record.explicit && (record.confidence >= 0.9 || record.count >= 2),
        paths::SOUL => record.explicit && (record.confidence >= 0.8 && record.count >= 1),
        paths::IDENTITY => record.confidence >= 0.7,
        paths::USER => record.confidence >= 0.75,
        paths::MEMORY => record.confidence >= 0.6,
        _ => false,
    }
}

fn build_deterministic_sections(state: &ProfileState) -> HashMap<String, String> {
    let mut by_doc: HashMap<String, Vec<&StatementRecord>> = HashMap::new();
    for record in state.statements.values().filter(|r| should_include(r)) {
        by_doc
            .entry(record.target_doc.clone())
            .or_default()
            .push(record);
    }

    let mut out = HashMap::new();
    for (doc, mut items) in by_doc {
        items.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.count.cmp(&a.count))
        });
        let mut lines = vec![
            "## Synthesized Profile (Managed)".to_string(),
            format!("_Updated: {} UTC_", Utc::now().format("%Y-%m-%d %H:%M:%S")),
            String::new(),
        ];
        for item in items.into_iter().take(12) {
            lines.push(format!(
                "- {} _(confidence {:.2}, seen {}x)_",
                item.statement, item.confidence, item.count
            ));
        }
        out.insert(doc, lines.join("\n"));
    }
    out
}

async fn llm_merge_sections(
    workspace: &Workspace,
    llm: &dyn LlmProvider,
    observations: &[Observation],
    deterministic_sections: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut current_docs = HashMap::new();
    for file in [
        paths::AGENTS,
        paths::IDENTITY,
        paths::SOUL,
        paths::USER,
        paths::MEMORY,
    ] {
        let content = workspace
            .read(file)
            .await
            .map(|d| d.content)
            .unwrap_or_default();
        current_docs.insert(
            file.to_string(),
            extract_managed_section(&content, file).unwrap_or_default(),
        );
    }

    let obs_json = serde_json::to_string(observations).map_err(|e| e.to_string())?;
    let current_json = serde_json::to_string(&current_docs).map_err(|e| e.to_string())?;
    let deterministic_json =
        serde_json::to_string(deterministic_sections).map_err(|e| e.to_string())?;

    let messages = vec![
        ChatMessage::system(
            "You merge profile observations into managed markdown sections for TitanClaw workspace docs. \
             Return ONLY JSON object mapping filenames to markdown strings for these keys exactly: \
             AGENTS.md, IDENTITY.md, SOUL.md, USER.md, MEMORY.md. Keep content factual, concise, and \
             conservative. Never rewrite manual sections; only generate managed section bodies.",
        ),
        ChatMessage::user(format!(
            "Current managed sections JSON:\n{}\n\nDeterministic fallback proposal JSON:\n{}\n\nNew observations JSON:\n{}\n\nReturn strict JSON only.",
            current_json, deterministic_json, obs_json
        )),
    ];
    let req = CompletionRequest::new(messages)
        .with_max_tokens(1200)
        .with_temperature(0.2);
    let resp = llm.complete(req).await.map_err(|e| e.to_string())?;
    parse_llm_section_json(&resp.content)
}

fn parse_llm_section_json(raw: &str) -> Result<HashMap<String, String>, String> {
    if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(raw) {
        return Ok(map);
    }
    let start = raw
        .find('{')
        .ok_or_else(|| "No JSON object found in LLM response".to_string())?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| "No JSON object end found in LLM response".to_string())?;
    serde_json::from_str::<HashMap<String, String>>(&raw[start..=end]).map_err(|e| e.to_string())
}

async fn append_observations(
    workspace: &Workspace,
    observations: &[Observation],
) -> Result<(), String> {
    for obs in observations {
        let line = serde_json::to_string(obs).map_err(|e| e.to_string())?;
        workspace
            .append(OBS_PATH, &line)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn load_state(workspace: &Workspace) -> Result<ProfileState, String> {
    let doc = workspace
        .read(STATE_PATH)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&doc.content).map_err(|e| e.to_string())
}

async fn save_state(workspace: &Workspace, state: &ProfileState) -> Result<(), String> {
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    workspace
        .write(STATE_PATH, &json)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn update_managed_section(
    workspace: &Workspace,
    path: &str,
    managed_body: &str,
) -> Result<(), String> {
    let existing = workspace
        .read(path)
        .await
        .map(|d| d.content)
        .unwrap_or_default();
    let updated = render_with_managed_section(&existing, path, managed_body)?;
    workspace
        .write(path, &updated)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn managed_begin_marker(path: &str) -> String {
    format!("{MANAGED_BEGIN_PREFIX} file={path} -->")
}

fn extract_managed_section(content: &str, path: &str) -> Result<String, String> {
    let begin = managed_begin_marker(path);
    let Some(start) = content.find(&begin) else {
        return Ok(String::new());
    };
    let after_begin = start + begin.len();
    let Some(end_rel) = content[after_begin..].find(MANAGED_END_MARKER) else {
        return Err(format!(
            "Managed section start found but end marker missing in {}",
            path
        ));
    };
    let body = &content[after_begin..after_begin + end_rel];
    Ok(body.trim().to_string())
}

fn render_with_managed_section(
    content: &str,
    path: &str,
    managed_body: &str,
) -> Result<String, String> {
    let begin = managed_begin_marker(path);
    let block = format!("{}\n{}\n{}", begin, managed_body.trim(), MANAGED_END_MARKER);

    let start_positions: Vec<_> = content.match_indices(&begin).collect();
    if start_positions.len() > 1 {
        return Err(format!("Multiple managed section starts found in {}", path));
    }
    let end_positions: Vec<_> = content.match_indices(MANAGED_END_MARKER).collect();
    if end_positions.len() > 1 {
        return Err(format!("Multiple managed section ends found in {}", path));
    }

    if let Some((start_idx, _)) = start_positions.first().copied() {
        let after_begin_idx = start_idx + begin.len();
        let Some(end_rel) = content[after_begin_idx..].find(MANAGED_END_MARKER) else {
            return Err(format!("Managed section end missing in {}", path));
        };
        let end_idx = after_begin_idx + end_rel + MANAGED_END_MARKER.len();
        let mut out = String::new();
        out.push_str(&content[..start_idx]);
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&block);
        out.push_str(&content[end_idx..]);
        Ok(out)
    } else if content.trim().is_empty() {
        Ok(block)
    } else {
        let mut out = content.trim_end().to_string();
        out.push_str("\n\n");
        out.push_str(&block);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_managed_block_insert_and_replace() {
        let path = paths::USER;
        let initial = "# User Context\n\nManual notes.";
        let rendered = render_with_managed_section(initial, path, "## Synth\n- A").unwrap();
        assert!(rendered.contains(MANAGED_END_MARKER));
        assert!(rendered.contains("Manual notes."));

        let replaced = render_with_managed_section(&rendered, path, "## Synth\n- B").unwrap();
        assert!(replaced.contains("- B"));
        assert!(!replaced.contains("- A"));
        assert!(replaced.contains("Manual notes."));
    }

    #[test]
    fn test_extract_observations_basic() {
        let evt = SynthesisEvent {
            user_id: "u".into(),
            thread_id: "t".into(),
            user_input: "Please keep docs synced and don't break onboarding".into(),
            assistant_response: "Implemented the fix.".into(),
            timestamp: Utc::now().to_rfc3339(),
        };
        let obs = extract_observations(&evt);
        assert!(!obs.is_empty());
        assert!(obs.iter().any(|o| o.target_doc == paths::AGENTS));
    }
}
