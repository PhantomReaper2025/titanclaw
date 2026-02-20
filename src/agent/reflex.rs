use std::sync::Arc;
use std::time::Duration;

use crate::db::Database;
use crate::llm::LlmProvider;
use crate::safety::SafetyLayer;
use crate::tools::ToolRegistry;
use crate::tools::builder::{
    BuildRequirement, BuilderConfig, Language, LlmSoftwareBuilder, SoftwareBuilder, SoftwareType,
};
use uuid::Uuid;

fn normalize_pattern(input: &str) -> String {
    input
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Spawn the Reflex compiler background ticker.
/// It periodically scans the job history for recurring prompts
/// and compiles them into WASM micro-skills.
pub fn spawn_reflex_compiler(
    store: Arc<dyn Database>,
    llm: Arc<dyn LlmProvider>,
    safety: Arc<SafetyLayer>,
    tools: Arc<ToolRegistry>,
    check_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(check_interval);
        // Skip immediate first tick
        ticker.tick().await;

        let builder = LlmSoftwareBuilder::new(BuilderConfig::default(), llm, safety, tools.clone());

        loop {
            ticker.tick().await;

            // Fetch job descriptions that appear 3 or more times, up to 5 patterns.
            match store.find_recurring_job_patterns(3, 5).await {
                Ok(patterns) => {
                    for description in patterns {
                        // Very naive check to avoid recompiling exactly the same pattern
                        // In reality, you'd match by vector embedding or semantic similarity
                        let all_tools = tools.list().await;
                        // For demonstration, let's assume we don't know the exact tool name
                        // but if we had metadata we'd skip. We'll proceed with compiling for now
                        // if we want a fresh Reflex.
                        // We will skip if the pattern is already a known tool name.
                        if all_tools.iter().any(|t| *t == description) {
                            continue;
                        }

                        tracing::info!(
                            "Reflex compiler triggered for recurring pattern: {}",
                            description
                        );

                        let safe_name = Uuid::new_v4()
                            .simple()
                            .to_string()
                            .chars()
                            .take(8)
                            .collect::<String>();
                        let req = BuildRequirement {
                            name: format!("reflex_{}", safe_name),
                            description: format!("A specialized tool to handle: {}", description),
                            software_type: SoftwareType::WasmTool,
                            language: Language::Rust,
                            input_spec: None,
                            output_spec: None,
                            dependencies: vec![],
                            capabilities: vec!["log".to_string()], // minimal caps
                        };

                        tracing::info!("Starting background compilation for Reflex: {}", req.name);
                        match builder.build(&req).await {
                            Ok(result) => {
                                if result.success {
                                    tracing::info!(
                                        "Successfully compiled Reflex MS (micro-skill) for pattern: {}",
                                        description
                                    );
                                    let normalized = normalize_pattern(&description);
                                    if let Err(e) =
                                        store.upsert_reflex_pattern(&normalized, &req.name).await
                                    {
                                        tracing::warn!(
                                            "Failed to persist reflex pattern '{}' -> '{}': {}",
                                            normalized,
                                            req.name,
                                            e
                                        );
                                    }
                                } else {
                                    tracing::warn!(
                                        "Failed to compile Reflex MS for pattern: {}",
                                        description
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!("Reflex compiler error: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to fetch recurring job patterns: {}", e);
                }
            }
        }
    })
}
