use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::agent::kernel_orchestrator::KernelOrchestrator;
use crate::context::JobContext;
use crate::tools::tool::{Tool, ToolError, ToolOutput, require_str};

pub struct KernelPatchTool {
    orchestrator: Arc<KernelOrchestrator>,
}

impl KernelPatchTool {
    pub fn new(orchestrator: Arc<KernelOrchestrator>) -> Self {
        Self { orchestrator }
    }
}

#[async_trait]
impl Tool for KernelPatchTool {
    fn name(&self) -> &str {
        "kernel_patch"
    }

    fn description(&self) -> &str {
        "List, approve, reject, and deploy kernel self-optimization patch proposals."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "approve", "reject", "deploy"],
                    "description": "Action to perform."
                },
                "patch_id": {
                    "type": "string",
                    "description": "Patch UUID required for approve/reject/deploy."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();
        let action = require_str(&params, "action")?;
        match action {
            "list" => {
                let patches = self.orchestrator.list_patches().await;
                let items: Vec<serde_json::Value> = patches
                    .into_iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id.to_string(),
                            "tool_name": p.tool_name,
                            "reason": p.reason,
                            "expected_speedup": p.expected_speedup,
                            "status": format!("{:?}", p.status),
                        })
                    })
                    .collect();
                Ok(ToolOutput::success(
                    serde_json::json!({
                        "count": items.len(),
                        "patches": items
                    }),
                    start.elapsed(),
                ))
            }
            "approve" => {
                let patch_id = parse_patch_id(&params)?;
                let ok = self.orchestrator.approve_patch(patch_id).await;
                Ok(ToolOutput::success(
                    serde_json::json!({
                        "patch_id": patch_id.to_string(),
                        "approved": ok
                    }),
                    start.elapsed(),
                ))
            }
            "reject" => {
                let patch_id = parse_patch_id(&params)?;
                let ok = self.orchestrator.reject_patch(patch_id).await;
                Ok(ToolOutput::success(
                    serde_json::json!({
                        "patch_id": patch_id.to_string(),
                        "rejected": ok
                    }),
                    start.elapsed(),
                ))
            }
            "deploy" => {
                let patch_id = parse_patch_id(&params)?;
                let deployed = self.orchestrator.deploy_patch_by_id(patch_id).await;
                match deployed {
                    Ok(()) => Ok(ToolOutput::success(
                        serde_json::json!({
                            "patch_id": patch_id.to_string(),
                            "deployed": true
                        }),
                        start.elapsed(),
                    )),
                    Err(e) => Err(ToolError::ExecutionFailed(e)),
                }
            }
            other => Err(ToolError::InvalidParameters(format!(
                "unknown action '{}'",
                other
            ))),
        }
    }

    fn requires_sanitization(&self) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        true
    }
}

fn parse_patch_id(params: &serde_json::Value) -> Result<Uuid, ToolError> {
    let raw = require_str(params, "patch_id")?;
    Uuid::parse_str(raw).map_err(|e| ToolError::InvalidParameters(format!("invalid patch_id: {e}")))
}
