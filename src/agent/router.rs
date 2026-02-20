//! Message routing to appropriate handlers.
//!
//! The router handles explicit commands (starting with `/`).
//! Natural language intent classification is handled by `IntentClassifier`
//! which uses LLM + tools instead of brittle pattern matching.

use crate::channels::IncomingMessage;
use regex::Regex;

/// Intent extracted from a message.
#[derive(Debug, Clone)]
pub enum MessageIntent {
    /// Create a new job.
    CreateJob {
        title: String,
        description: String,
        category: Option<String>,
    },
    /// Check status of a job.
    CheckJobStatus { job_id: Option<String> },
    /// Cancel a job.
    CancelJob { job_id: String },
    /// List jobs.
    ListJobs { filter: Option<String> },
    /// Help with a stuck job.
    HelpJob { job_id: String },
    /// General conversation/question.
    Chat { content: String },
    /// System command.
    Command { command: String, args: Vec<String> },
    /// Unknown intent.
    Unknown,
}

/// Routes messages to appropriate handlers based on explicit commands.
///
/// For natural language messages, use `IntentClassifier` instead.
pub struct Router {
    /// Command prefix (e.g., "/" or "!")
    command_prefix: String,
}

impl Router {
    /// Create a new router.
    pub fn new() -> Self {
        Self {
            command_prefix: "/".to_string(),
        }
    }

    /// Set the command prefix.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.command_prefix = prefix.into();
        self
    }

    /// Check if a message is an explicit command.
    pub fn is_command(&self, message: &IncomingMessage) -> bool {
        message.content.trim().starts_with(&self.command_prefix)
    }

    /// Route an explicit command to determine its intent.
    ///
    /// Returns `None` if the message is not a command.
    /// For non-commands, use `IntentClassifier::classify()` instead.
    pub fn route_command(&self, message: &IncomingMessage) -> Option<MessageIntent> {
        let content = message.content.trim();

        if content.starts_with(&self.command_prefix) {
            Some(self.parse_command(content))
        } else {
            None
        }
    }

    /// Route a small set of high-confidence natural-language intents.
    ///
    /// This is a deterministic fast-path for common job operations so we can
    /// bypass an LLM call for repeated operational requests.
    pub fn route_fast_path_nl(&self, message: &IncomingMessage) -> Option<MessageIntent> {
        if self.is_command(message) {
            return None;
        }

        let raw = message.content.trim();
        if raw.is_empty() {
            return None;
        }

        let normalized = raw.to_lowercase();
        let normalized = normalized.trim();

        // list jobs / show jobs variants
        let list_phrases = [
            "list jobs",
            "list my jobs",
            "show jobs",
            "show my jobs",
            "what jobs are running",
            "what are my jobs",
        ];
        if list_phrases.contains(&normalized) {
            return Some(MessageIntent::ListJobs { filter: None });
        }

        // create job <description>
        let create_prefix = "create job ";
        if normalized.starts_with(create_prefix) {
            let original_rest = raw.get(create_prefix.len()..).unwrap_or("").trim();
            if !original_rest.is_empty() {
                return Some(MessageIntent::CreateJob {
                    title: original_rest.to_string(),
                    description: original_rest.to_string(),
                    category: None,
                });
            }
        }

        // status of job <id> / check job <id>
        let status_re =
            Regex::new(r"^(?:status(?: of)?|check(?: status)?(?: of)?) job ([^\s]+)$").ok()?;
        if let Some(caps) = status_re.captures(normalized)
            && let Some(job_id) = caps.get(1).map(|m| m.as_str().to_string())
        {
            return Some(MessageIntent::CheckJobStatus {
                job_id: Some(job_id),
            });
        }

        // cancel/stop job <id>
        let cancel_re = Regex::new(r"^(?:cancel|stop) job ([^\s]+)$").ok()?;
        if let Some(caps) = cancel_re.captures(normalized)
            && let Some(job_id) = caps.get(1).map(|m| m.as_str().to_string())
        {
            return Some(MessageIntent::CancelJob { job_id });
        }

        // help/unstick job <id>
        let help_re = Regex::new(r"^(?:help|unstick) job ([^\s]+)$").ok()?;
        if let Some(caps) = help_re.captures(normalized)
            && let Some(job_id) = caps.get(1).map(|m| m.as_str().to_string())
        {
            return Some(MessageIntent::HelpJob { job_id });
        }

        None
    }

    fn parse_command(&self, content: &str) -> MessageIntent {
        let without_prefix = content
            .strip_prefix(&self.command_prefix)
            .unwrap_or(content);
        let parts: Vec<&str> = without_prefix.split_whitespace().collect();

        match parts.first().map(|s| s.to_lowercase()).as_deref() {
            Some("job") | Some("create") => {
                let rest = parts[1..].join(" ");
                MessageIntent::CreateJob {
                    title: rest.clone(),
                    description: rest,
                    category: None,
                }
            }
            Some("status") => {
                let job_id = parts.get(1).map(|s| s.to_string());
                MessageIntent::CheckJobStatus { job_id }
            }
            Some("cancel") => {
                if let Some(job_id) = parts.get(1) {
                    MessageIntent::CancelJob {
                        job_id: job_id.to_string(),
                    }
                } else {
                    MessageIntent::Unknown
                }
            }
            Some("list") | Some("jobs") => {
                let filter = parts.get(1).map(|s| s.to_string());
                MessageIntent::ListJobs { filter }
            }
            Some("help") => {
                if let Some(job_id) = parts.get(1) {
                    MessageIntent::HelpJob {
                        job_id: job_id.to_string(),
                    }
                } else {
                    MessageIntent::Command {
                        command: "help".to_string(),
                        args: vec![],
                    }
                }
            }
            Some(cmd) => MessageIntent::Command {
                command: cmd.to_string(),
                args: parts[1..].iter().map(|s| s.to_string()).collect(),
            },
            None => MessageIntent::Unknown,
        }
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_routing() {
        let router = Router::new();

        let msg = IncomingMessage::new("test", "user", "/status abc-123");
        let intent = router.route_command(&msg);

        assert!(matches!(intent, Some(MessageIntent::CheckJobStatus { .. })));
    }

    #[test]
    fn test_is_command() {
        let router = Router::new();

        let cmd_msg = IncomingMessage::new("test", "user", "/status");
        assert!(router.is_command(&cmd_msg));

        let chat_msg = IncomingMessage::new("test", "user", "Hello there");
        assert!(!router.is_command(&chat_msg));
    }

    #[test]
    fn test_non_command_returns_none() {
        let router = Router::new();

        // Natural language messages return None - they should use IntentClassifier
        let msg = IncomingMessage::new("test", "user", "Can you create a website for me?");
        assert!(router.route_command(&msg).is_none());

        let msg2 = IncomingMessage::new("test", "user", "Hello, how are you?");
        assert!(router.route_command(&msg2).is_none());
    }

    #[test]
    fn test_command_create_job() {
        let router = Router::new();

        let msg = IncomingMessage::new("test", "user", "/job build a website");
        let intent = router.route_command(&msg);

        match intent {
            Some(MessageIntent::CreateJob { title, .. }) => {
                assert_eq!(title, "build a website");
            }
            _ => panic!("Expected CreateJob intent"),
        }
    }

    #[test]
    fn test_command_list_jobs() {
        let router = Router::new();

        let msg = IncomingMessage::new("test", "user", "/list active");
        let intent = router.route_command(&msg);

        match intent {
            Some(MessageIntent::ListJobs { filter }) => {
                assert_eq!(filter, Some("active".to_string()));
            }
            _ => panic!("Expected ListJobs intent"),
        }
    }

    #[test]
    fn test_fast_path_list_jobs() {
        let router = Router::new();
        let msg = IncomingMessage::new("test", "user", "show my jobs");
        let intent = router.route_fast_path_nl(&msg);
        assert!(matches!(intent, Some(MessageIntent::ListJobs { .. })));
    }

    #[test]
    fn test_fast_path_status_job() {
        let router = Router::new();
        let msg = IncomingMessage::new("test", "user", "status of job abc-123");
        let intent = router.route_fast_path_nl(&msg);
        assert!(matches!(
            intent,
            Some(MessageIntent::CheckJobStatus {
                job_id: Some(ref id)
            }) if id == "abc-123"
        ));
    }

    #[test]
    fn test_fast_path_cancel_job() {
        let router = Router::new();
        let msg = IncomingMessage::new("test", "user", "cancel job job-789");
        let intent = router.route_fast_path_nl(&msg);
        assert!(matches!(
            intent,
            Some(MessageIntent::CancelJob { ref job_id }) if job_id == "job-789"
        ));
    }

    #[test]
    fn test_fast_path_create_job() {
        let router = Router::new();
        let msg = IncomingMessage::new("test", "user", "create job build auth middleware");
        let intent = router.route_fast_path_nl(&msg);
        assert!(matches!(
            intent,
            Some(MessageIntent::CreateJob { ref title, .. }) if title == "build auth middleware"
        ));
    }

    #[test]
    fn test_fast_path_ignores_slash_commands() {
        let router = Router::new();
        let msg = IncomingMessage::new("test", "user", "/list");
        assert!(router.route_fast_path_nl(&msg).is_none());
    }
}
