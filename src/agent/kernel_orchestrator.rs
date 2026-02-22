use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::agent::kernel_monitor::{KernelMonitor, PatchProposal, PatchStatus, ToolPerfStats};
use crate::context::JobContext;
use crate::tools::ToolRegistry;

#[derive(Debug, Clone)]
pub struct KernelOrchestratorConfig {
    pub enabled: bool,
    pub interval: Duration,
    pub auto_approve: bool,
    pub auto_deploy: bool,
}

#[derive(Clone)]
pub struct KernelOrchestrator {
    config: KernelOrchestratorConfig,
    monitor: Arc<KernelMonitor>,
    tools: Arc<ToolRegistry>,
}

impl KernelOrchestrator {
    pub fn new(
        config: KernelOrchestratorConfig,
        monitor: Arc<KernelMonitor>,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            config,
            monitor,
            tools,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn interval(&self) -> Duration {
        self.config.interval
    }

    pub async fn record_tool_execution(&self, tool_name: &str, duration: Duration, success: bool) {
        if !self.config.enabled {
            return;
        }
        self.monitor
            .record_execution(tool_name, duration, success)
            .await;
    }

    pub async fn tick(&self) {
        if !self.config.enabled {
            return;
        }

        let bottlenecks = self.monitor.detect_bottlenecks().await;
        if bottlenecks.is_empty() {
            return;
        }

        for stats in bottlenecks {
            if self
                .monitor
                .has_open_proposal_for_tool(&stats.tool_name)
                .await
            {
                continue;
            }
            let proposal = self.make_proposal(&stats);
            self.monitor.propose_patch(proposal).await;
        }

        if self.config.auto_approve {
            let pending = self.monitor.list_pending_patches().await;
            for patch in pending {
                let _ = self.monitor.approve_patch(patch.id).await;
            }
        }

        if self.config.auto_deploy {
            self.deploy_approved_patches().await;
        }
    }

    pub async fn deploy_approved_patches(&self) {
        let approved = self.monitor.get_approved_patches().await;
        for patch in approved {
            if let Err(e) = self.deploy_patch(&patch).await {
                let _ = self.monitor.mark_failed(patch.id, e).await;
            } else {
                let _ = self.monitor.mark_deployed(patch.id).await;
            }
        }
    }

    pub async fn list_patches(&self) -> Vec<PatchProposal> {
        self.monitor.all_patches().await
    }

    pub async fn approve_patch(&self, patch_id: Uuid) -> bool {
        self.monitor.approve_patch(patch_id).await
    }

    pub async fn reject_patch(&self, patch_id: Uuid) -> bool {
        self.monitor.reject_patch(patch_id).await
    }

    pub async fn deploy_patch_by_id(&self, patch_id: Uuid) -> Result<(), String> {
        let patch = self
            .monitor
            .get_patch(patch_id)
            .await
            .ok_or_else(|| format!("patch {} not found", patch_id))?;
        if patch.status != PatchStatus::Approved {
            return Err(format!(
                "patch {} is not approved (status: {:?})",
                patch_id, patch.status
            ));
        }
        match self.deploy_patch(&patch).await {
            Ok(()) => {
                let _ = self.monitor.mark_deployed(patch_id).await;
                Ok(())
            }
            Err(e) => {
                let _ = self.monitor.mark_failed(patch_id, e.clone()).await;
                Err(e)
            }
        }
    }

    async fn deploy_patch(&self, patch: &PatchProposal) -> Result<(), String> {
        let jit = self
            .tools
            .get("jit_wasm_run")
            .await
            .ok_or_else(|| "jit_wasm_run is not registered".to_string())?;

        let params = serde_json::json!({
            "rust_code": patch.proposed_rust_code,
            "input_json": "{\"probe\":true}"
        });

        let ctx = JobContext::new(
            format!("kernel_patch_{}", patch.tool_name),
            "Kernel patch validation run",
        );

        let result = jit.execute(params, &ctx).await.map_err(|e| e.to_string())?;
        if result.result.get("error").is_some() {
            return Err(format!(
                "JIT patch reported error: {}",
                result
                    .result
                    .get("error")
                    .unwrap_or(&serde_json::Value::Null)
            ));
        }

        Ok(())
    }

    fn make_proposal(&self, stats: &ToolPerfStats) -> PatchProposal {
        let reason = format!(
            "avg {:.1}ms exceeds threshold with {} runs (p95 {:.1}ms, max {:.1}ms)",
            stats.avg_duration_ms,
            stats.total_executions,
            stats.p95_duration_ms,
            stats.max_duration_ms
        );
        PatchProposal {
            id: Uuid::new_v4(),
            tool_name: stats.tool_name.clone(),
            reason,
            proposed_rust_code: generated_patch_stub(&stats.tool_name),
            expected_speedup: 1.15,
            status: PatchStatus::PendingApproval,
        }
    }
}

fn generated_patch_stub(tool_name: &str) -> String {
    format!(
        r#"
fn run_inner(input_ptr: *const u8, input_len: usize) -> Result<String, String> {{
    let input = if input_ptr.is_null() || input_len == 0 {{
        String::new()
    }} else {{
        unsafe {{
            let slice = std::slice::from_raw_parts(input_ptr, input_len);
            String::from_utf8(slice.to_vec()).map_err(|e| e.to_string())?
        }}
    }};
    log_info("kernel patch probe for {tool_name}");
    Ok(serde_json::json!({{
        "tool": "{tool_name}",
        "optimized": true,
        "input_len": input.len()
    }}).to_string())
}}
"#
    )
}
