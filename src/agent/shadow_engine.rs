use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, Semaphore};

use crate::llm::{ChatMessage, CompletionRequest, LlmProvider};

#[derive(Debug, Clone)]
struct ShadowCacheEntry {
    response: String,
    created_at: Instant,
}

#[derive(Clone)]
pub struct ShadowEngine {
    enabled: bool,
    llm: Arc<dyn LlmProvider>,
    cache: Arc<RwLock<HashMap<String, ShadowCacheEntry>>>,
    ttl: Duration,
    max_predictions: usize,
    min_input_chars: usize,
    semaphore: Arc<Semaphore>,
}

impl ShadowEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        enabled: bool,
        llm: Arc<dyn LlmProvider>,
        ttl: Duration,
        max_predictions: usize,
        min_input_chars: usize,
        max_parallel: usize,
    ) -> Self {
        Self {
            enabled,
            llm,
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl,
            max_predictions,
            min_input_chars,
            semaphore: Arc::new(Semaphore::new(max_parallel.max(1))),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub async fn try_get(&self, input: &str) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let key = normalize(input);
        let mut cache = self.cache.write().await;
        if let Some(entry) = cache.get(&key) {
            if entry.created_at.elapsed() <= self.ttl {
                return Some(entry.response.clone());
            }
        }
        cache.remove(&key);
        None
    }

    pub fn spawn_speculation(&self, user_input: &str, assistant_response: &str) {
        if !self.enabled || user_input.chars().count() < self.min_input_chars {
            return;
        }

        let this = self.clone();
        let user_input = user_input.to_string();
        let assistant_response = assistant_response.to_string();
        tokio::spawn(async move {
            if let Err(e) = this
                .speculate_for_next_turn(&user_input, &assistant_response)
                .await
            {
                tracing::debug!("Shadow speculation failed: {}", e);
            }
        });
    }

    pub async fn prune_expired(&self) {
        if !self.enabled {
            return;
        }
        let mut cache = self.cache.write().await;
        cache.retain(|_, v| v.created_at.elapsed() <= self.ttl);
    }

    async fn speculate_for_next_turn(
        &self,
        user_input: &str,
        assistant_response: &str,
    ) -> Result<(), String> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| e.to_string())?;
        let _permit = permit;

        let predictions = self
            .predict_followup_prompts(user_input, assistant_response)
            .await?;
        if predictions.is_empty() {
            return Ok(());
        }

        for prompt in predictions.into_iter().take(self.max_predictions.max(1)) {
            let prediction = prompt.trim();
            if prediction.is_empty() {
                continue;
            }
            let key = normalize(prediction);
            if self.cache.read().await.contains_key(&key) {
                continue;
            }
            if let Ok(response) = self.precompute_response(prediction).await {
                self.cache.write().await.insert(
                    key,
                    ShadowCacheEntry {
                        response,
                        created_at: Instant::now(),
                    },
                );
            }
        }

        Ok(())
    }

    async fn predict_followup_prompts(
        &self,
        user_input: &str,
        assistant_response: &str,
    ) -> Result<Vec<String>, String> {
        let prompt = format!(
            "Given this chat turn, predict up to {} likely short next user messages.\n\
             Return ONLY valid JSON as an array of strings.\n\n\
             User: {}\nAssistant: {}",
            self.max_predictions.max(1),
            user_input,
            assistant_response
        );

        let request = CompletionRequest::new(vec![
            ChatMessage::system(
                "You predict likely follow-up user prompts for an assistant. Return JSON only.",
            ),
            ChatMessage::user(prompt),
        ])
        .with_temperature(0.2)
        .with_max_tokens(180);

        let response = self
            .llm
            .complete(request)
            .await
            .map_err(|e| e.to_string())?;
        let raw = response.content.trim();
        if let Ok(v) = serde_json::from_str::<Vec<String>>(raw) {
            return Ok(v);
        }

        // Fallback for models that return plain text bullets.
        let mut items = Vec::new();
        for line in raw.lines() {
            let s = line
                .trim()
                .trim_start_matches('-')
                .trim_start_matches('*')
                .trim();
            if !s.is_empty() {
                items.push(s.to_string());
            }
        }
        Ok(items)
    }

    async fn precompute_response(&self, predicted_prompt: &str) -> Result<String, String> {
        let request = CompletionRequest::new(vec![
            ChatMessage::system(
                "You are a coding assistant. Give concise, practical answers. \
                 Do not use tools. If uncertain, state assumptions briefly.",
            ),
            ChatMessage::user(predicted_prompt.to_string()),
        ])
        .with_temperature(0.4)
        .with_max_tokens(700);

        let response = self
            .llm
            .complete(request)
            .await
            .map_err(|e| e.to_string())?;
        Ok(response.content)
    }
}

fn normalize(input: &str) -> String {
    input
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
