//! OpenCode bridge for sandboxed execution.
//!
//! Spawns the `opencode` CLI inside a Docker container and streams output
//! back to the orchestrator as job events. Supports follow-up prompts using
//! `opencode run --continue`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::error::WorkerError;
use crate::worker::api::{CompletionReport, JobEventPayload, PromptResponse, WorkerHttpClient};

/// Configuration for the OpenCode bridge runtime.
pub struct OpenCodeBridgeConfig {
    pub job_id: Uuid,
    pub orchestrator_url: String,
    pub max_turns: u32,
    pub model: String,
    pub timeout: Duration,
}

/// The OpenCode bridge runtime.
pub struct OpenCodeBridgeRuntime {
    config: OpenCodeBridgeConfig,
    client: Arc<WorkerHttpClient>,
}

impl OpenCodeBridgeRuntime {
    /// Create a new bridge runtime.
    ///
    /// Reads `IRONCLAW_WORKER_TOKEN` from the environment for auth.
    pub fn new(config: OpenCodeBridgeConfig) -> Result<Self, WorkerError> {
        let client = Arc::new(WorkerHttpClient::from_env(
            config.orchestrator_url.clone(),
            config.job_id,
        )?);

        Ok(Self { config, client })
    }

    /// Run the bridge: fetch job, spawn OpenCode, stream events, handle follow-ups.
    pub async fn run(&self) -> Result<(), WorkerError> {
        let job = self.client.get_job().await?;

        tracing::info!(
            job_id = %self.config.job_id,
            "Starting OpenCode bridge for: {}",
            truncate(&job.description, 100)
        );

        // Fetch credentials for child process injection.
        let credentials = self.client.fetch_credentials().await?;
        let mut extra_env = HashMap::new();
        for cred in &credentials {
            extra_env.insert(cred.env_var.clone(), cred.value.clone());
        }
        if !extra_env.is_empty() {
            tracing::info!(
                job_id = %self.config.job_id,
                "Fetched {} credential(s) for child process injection",
                extra_env.len()
            );
        }

        // Honor explicit OPENCODE_MODEL env first (injected by orchestrator), then config model.
        let model = std::env::var("OPENCODE_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| self.config.model.clone());

        self.client
            .report_status(&crate::worker::api::StatusUpdate {
                state: "running".to_string(),
                message: Some(format!("Spawning OpenCode (model: {})", model)),
                iteration: 0,
            })
            .await?;
        self.report_event(
            "status",
            &serde_json::json!({
                "message": format!("Starting OpenCode bridge (model: {})", model),
            }),
        )
        .await;

        // Initial session run.
        if let Err(e) = self
            .run_opencode_session(&job.description, false, &model, &extra_env)
            .await
        {
            tracing::error!(job_id = %self.config.job_id, "OpenCode session failed: {}", e);
            self.client
                .report_complete(&CompletionReport {
                    success: false,
                    message: Some(format!("OpenCode failed: {}", e)),
                    iterations: 1,
                })
                .await?;
            return Ok(());
        }

        // Follow-up loop.
        let mut iteration = 1u32;
        loop {
            if iteration >= self.config.max_turns {
                tracing::warn!(
                    job_id = %self.config.job_id,
                    max_turns = self.config.max_turns,
                    "Stopping OpenCode bridge after max_turns"
                );
                break;
            }

            match self.poll_for_prompt().await {
                Ok(Some(prompt)) => {
                    if prompt.done {
                        tracing::info!(job_id = %self.config.job_id, "Orchestrator signaled done");
                        break;
                    }

                    iteration += 1;
                    tracing::info!(
                        job_id = %self.config.job_id,
                        "Got follow-up prompt, continuing OpenCode session"
                    );
                    if let Err(e) = self
                        .run_opencode_session(&prompt.content, true, &model, &extra_env)
                        .await
                    {
                        tracing::error!(
                            job_id = %self.config.job_id,
                            "Follow-up OpenCode session failed: {}", e
                        );
                        self.report_event(
                            "status",
                            &serde_json::json!({
                                "message": format!("Follow-up session failed: {}", e),
                            }),
                        )
                        .await;
                    }
                }
                Ok(None) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => {
                    tracing::warn!(
                        job_id = %self.config.job_id,
                        "Prompt polling error: {}", e
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }

        self.client
            .report_complete(&CompletionReport {
                success: true,
                message: Some("OpenCode session completed".to_string()),
                iterations: iteration,
            })
            .await?;
        self.report_event(
            "status",
            &serde_json::json!({
                "message": "OpenCode bridge completed",
            }),
        )
        .await;

        Ok(())
    }

    async fn run_opencode_session(
        &self,
        prompt: &str,
        resume: bool,
        model: &str,
        extra_env: &HashMap<String, String>,
    ) -> Result<(), WorkerError> {
        self.report_event(
            "status",
            &serde_json::json!({
                "message": if resume { "Continuing OpenCode session..." } else { "Launching OpenCode session..." },
                "resume": resume,
                "model": model,
            }),
        )
        .await;

        let mut cmd = Command::new("opencode");
        cmd.arg("run")
            .arg("--format")
            .arg("json")
            .arg("--model")
            .arg(model)
            .arg(prompt);

        if resume {
            cmd.arg("--continue");
        }

        cmd.envs(extra_env);
        cmd.env("HOME", "/home/sandbox");
        cmd.env("XDG_CONFIG_HOME", "/home/sandbox/.config");
        cmd.env("XDG_DATA_HOME", "/home/sandbox/.local/share");
        cmd.env("XDG_CACHE_HOME", "/home/sandbox/.cache");

        cmd.current_dir("/workspace")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| WorkerError::ExecutionFailed {
            reason: format!("failed to spawn opencode: {}", e),
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| WorkerError::ExecutionFailed {
                reason: "failed to capture opencode stdout".to_string(),
            })?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| WorkerError::ExecutionFailed {
                reason: "failed to capture opencode stderr".to_string(),
            })?;

        let client_for_stderr = Arc::clone(&self.client);
        let job_id = self.config.job_id;
        let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let stderr_tail_for_task = Arc::clone(&stderr_tail);
        let stderr_handle = tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(job_id = %job_id, "opencode stderr: {}", line);
                if let Ok(mut guard) = stderr_tail_for_task.lock() {
                    guard.push_back(line.clone());
                    while guard.len() > 20 {
                        let _ = guard.pop_front();
                    }
                }
                let payload = JobEventPayload {
                    event_type: "status".to_string(),
                    data: serde_json::json!({ "message": line }),
                };
                client_for_stderr.post_event(&payload).await;
            }
        });

        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(payloads) = opencode_json_to_payloads(&v) {
                    for payload in payloads {
                        self.report_event(&payload.event_type, &payload.data).await;
                    }
                } else {
                    self.report_event(
                        "status",
                        &serde_json::json!({
                            "message": line,
                        }),
                    )
                    .await;
                }
            } else {
                self.report_event(
                    "status",
                    &serde_json::json!({
                        "message": line,
                    }),
                )
                .await;
            }
        }

        self.report_event(
            "status",
            &serde_json::json!({
                "message": "OpenCode process finished emitting stdout; waiting for exit status...",
            }),
        )
        .await;

        let status = child
            .wait()
            .await
            .map_err(|e| WorkerError::ExecutionFailed {
                reason: format!("failed waiting for opencode: {}", e),
            })?;

        let _ = stderr_handle.await;

        if !status.success() {
            let code = status.code().unwrap_or(-1);
            let stderr_excerpt = stderr_tail
                .lock()
                .ok()
                .map(|guard| {
                    guard
                        .iter()
                        .rev()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default();

            let reason = if stderr_excerpt.is_empty() {
                format!("opencode exited with code {}", code)
            } else {
                format!(
                    "opencode exited with code {} (stderr: {})",
                    code, stderr_excerpt
                )
            };
            self.report_event(
                "status",
                &serde_json::json!({
                    "message": reason.clone(),
                    "exit_code": code,
                }),
            )
            .await;
            return Err(WorkerError::ExecutionFailed { reason });
        }

        self.report_event(
            "result",
            &serde_json::json!({
                "status": "completed",
            }),
        )
        .await;

        Ok(())
    }

    async fn report_event(&self, event_type: &str, data: &serde_json::Value) {
        let payload = JobEventPayload {
            event_type: event_type.to_string(),
            data: data.clone(),
        };
        self.client.post_event(&payload).await;
    }

    async fn poll_for_prompt(&self) -> Result<Option<PromptResponse>, WorkerError> {
        self.client.poll_prompt().await
    }
}

fn opencode_json_to_payloads(v: &serde_json::Value) -> Option<Vec<JobEventPayload>> {
    let mut payloads = Vec::new();
    let event_type = v
        .get("type")
        .or_else(|| v.get("event"))
        .and_then(|x| x.as_str());
    let role = v
        .get("role")
        .or_else(|| v.get("author").and_then(|a| a.get("role")))
        .and_then(|x| x.as_str());
    let content = v
        .get("content")
        .or_else(|| v.get("message").and_then(|m| m.get("content")))
        .or_else(|| v.get("delta"))
        .or_else(|| v.get("text"));

    match (event_type, role) {
        (Some("message"), Some("assistant")) => {
            if let Some(text) = extract_text(content) {
                payloads.push(JobEventPayload {
                    event_type: "message".to_string(),
                    data: serde_json::json!({
                        "role": "assistant",
                        "content": text,
                    }),
                });
            }
        }
        (Some("assistant"), _) | (Some("assistant_message"), _) | (Some("output_text"), _) => {
            if let Some(text) = extract_text(content) {
                payloads.push(JobEventPayload {
                    event_type: "message".to_string(),
                    data: serde_json::json!({
                        "role": "assistant",
                        "content": text,
                    }),
                });
            }
        }
        (Some("delta"), _) | (Some("content_delta"), _) => {
            if let Some(text) = extract_text(content) {
                payloads.push(JobEventPayload {
                    event_type: "status".to_string(),
                    data: serde_json::json!({
                        "message": format!("stream: {}", text),
                    }),
                });
            }
        }
        (Some("tool_use"), _) => {
            payloads.push(JobEventPayload {
                event_type: "tool_use".to_string(),
                data: serde_json::json!({
                    "tool_name": v.get("name").or_else(|| v.get("tool_name")),
                    "input": v.get("input").or_else(|| v.get("arguments")),
                }),
            });
        }
        (Some("tool_result"), _) => {
            let tool_name = v
                .get("tool_name")
                .or_else(|| v.get("name"))
                .cloned()
                .unwrap_or(serde_json::Value::String("unknown".to_string()));
            payloads.push(JobEventPayload {
                event_type: "tool_result".to_string(),
                data: serde_json::json!({
                    "tool_name": tool_name,
                    "output": stringify_json(v.get("output").or_else(|| v.get("content"))),
                }),
            });
        }
        (Some("result"), _) => {
            payloads.push(JobEventPayload {
                event_type: "result".to_string(),
                data: serde_json::json!({
                    "status": "completed",
                    "output": stringify_json(v.get("output").or_else(|| v.get("content"))),
                }),
            });
        }
        _ => {
            if let Some(text) = extract_text(content) {
                payloads.push(JobEventPayload {
                    event_type: "status".to_string(),
                    data: serde_json::json!({ "message": text }),
                });
            } else {
                payloads.push(JobEventPayload {
                    event_type: "status".to_string(),
                    data: serde_json::json!({ "message": v }),
                });
            }
        }
    }

    if payloads.is_empty() {
        None
    } else {
        Some(payloads)
    }
}

fn stringify_json(v: Option<&serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
        None => None,
    }
}

fn extract_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(items)) => {
            let mut acc = String::new();
            for item in items {
                if let Some(s) = item.as_str() {
                    if !acc.is_empty() {
                        acc.push('\n');
                    }
                    acc.push_str(s);
                } else if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    if !acc.is_empty() {
                        acc.push('\n');
                    }
                    acc.push_str(text);
                }
            }
            if acc.is_empty() { None } else { Some(acc) }
        }
        Some(v) => Some(v.to_string()),
        None => None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::opencode_json_to_payloads;

    #[test]
    fn maps_assistant_message_event() {
        let event = serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": "hello"
        });
        let payloads = opencode_json_to_payloads(&event).expect("payloads");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].event_type, "message");
        assert_eq!(
            payloads[0].data.get("content").and_then(|v| v.as_str()),
            Some("hello")
        );
    }

    #[test]
    fn maps_tool_use_event() {
        let event = serde_json::json!({
            "type": "tool_use",
            "name": "shell",
            "input": { "command": "ls" }
        });
        let payloads = opencode_json_to_payloads(&event).expect("payloads");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].event_type, "tool_use");
    }
}
