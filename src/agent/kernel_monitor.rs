//! Kernel Monitor: Self-Modifying Engine.
//!
//! Watches tool execution performance and detects bottlenecks.
//! When a tool is consistently slow, it generates a WASM replacement
//! candidate and, after human approval, hot-swaps it into the runtime.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::db::Database;

/// A recorded tool execution sample.
#[derive(Debug, Clone)]
pub struct ToolExecSample {
    pub tool_name: String,
    pub duration: Duration,
    pub success: bool,
    pub timestamp: Instant,
}

/// Performance statistics for a single tool.
#[derive(Debug, Clone)]
pub struct ToolPerfStats {
    pub tool_name: String,
    pub total_executions: u64,
    pub total_failures: u64,
    pub avg_duration_ms: f64,
    pub p95_duration_ms: f64,
    pub max_duration_ms: f64,
}

/// A proposed runtime patch for a slow tool.
#[derive(Debug, Clone)]
pub struct PatchProposal {
    pub id: Uuid,
    pub tool_name: String,
    pub reason: String,
    pub proposed_rust_code: String,
    pub expected_speedup: f64,
    pub status: PatchStatus,
}

/// Status of a proposed patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchStatus {
    /// Waiting for human approval.
    PendingApproval,
    /// Approved, ready to compile and deploy.
    Approved,
    /// Rejected by human veto.
    Rejected,
    /// Successfully compiled and deployed.
    Deployed,
    /// Compilation or deployment failed.
    Failed(String),
}

/// The Kernel Monitor tracks tool performance and proposes optimizations.
pub struct KernelMonitor {
    /// Rolling window of execution samples per tool.
    samples: Arc<RwLock<HashMap<String, Vec<ToolExecSample>>>>,
    /// Pending patch proposals awaiting human approval.
    pending_patches: Arc<RwLock<Vec<PatchProposal>>>,
    /// Threshold: tools averaging above this are considered slow (ms).
    slow_threshold_ms: f64,
    /// Minimum sample count before analyzing a tool.
    min_samples: usize,
    /// Maximum samples to retain per tool.
    max_samples_per_tool: usize,
    /// Database for persisting proposals.
    _db: Option<Arc<dyn Database>>,
}

impl KernelMonitor {
    /// Create a new kernel monitor.
    pub fn new() -> Self {
        Self {
            samples: Arc::new(RwLock::new(HashMap::new())),
            pending_patches: Arc::new(RwLock::new(Vec::new())),
            slow_threshold_ms: 5000.0,
            min_samples: 10,
            max_samples_per_tool: 100,
            _db: None,
        }
    }

    /// Attach a database for persisting patch proposals.
    pub fn with_db(mut self, db: Arc<dyn Database>) -> Self {
        self._db = Some(db);
        self
    }

    /// Configure the slow tool threshold.
    pub fn with_threshold(mut self, threshold_ms: f64) -> Self {
        self.slow_threshold_ms = threshold_ms;
        self
    }

    /// Record a tool execution sample.
    pub async fn record_execution(&self, tool_name: &str, duration: Duration, success: bool) {
        let mut samples = self.samples.write().await;
        let entry = samples
            .entry(tool_name.to_string())
            .or_insert_with(Vec::new);

        entry.push(ToolExecSample {
            tool_name: tool_name.to_string(),
            duration,
            success,
            timestamp: Instant::now(),
        });

        // Trim to max window size
        if entry.len() > self.max_samples_per_tool {
            let excess = entry.len() - self.max_samples_per_tool;
            entry.drain(..excess);
        }
    }

    /// Compute performance statistics for all tracked tools.
    pub async fn compute_stats(&self) -> Vec<ToolPerfStats> {
        let samples = self.samples.read().await;
        let mut stats = Vec::new();

        for (name, tool_samples) in samples.iter() {
            if tool_samples.is_empty() {
                continue;
            }

            let total = tool_samples.len() as u64;
            let failures = tool_samples.iter().filter(|s| !s.success).count() as u64;
            let mut durations_ms: Vec<f64> = tool_samples
                .iter()
                .map(|s| s.duration.as_secs_f64() * 1000.0)
                .collect();
            durations_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            let avg = durations_ms.iter().sum::<f64>() / durations_ms.len() as f64;
            let p95_idx = ((durations_ms.len() as f64) * 0.95) as usize;
            let p95 = durations_ms
                .get(p95_idx.min(durations_ms.len() - 1))
                .copied()
                .unwrap_or(0.0);
            let max = durations_ms.last().copied().unwrap_or(0.0);

            stats.push(ToolPerfStats {
                tool_name: name.clone(),
                total_executions: total,
                total_failures: failures,
                avg_duration_ms: avg,
                p95_duration_ms: p95,
                max_duration_ms: max,
            });
        }

        stats
    }

    /// Detect tools that are consistently slow and return them sorted by avg latency.
    pub async fn detect_bottlenecks(&self) -> Vec<ToolPerfStats> {
        let stats = self.compute_stats().await;
        let mut slow: Vec<ToolPerfStats> = stats
            .into_iter()
            .filter(|s| {
                s.total_executions >= self.min_samples as u64
                    && s.avg_duration_ms > self.slow_threshold_ms
            })
            .collect();
        slow.sort_by(|a, b| {
            b.avg_duration_ms
                .partial_cmp(&a.avg_duration_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        slow
    }

    /// Submit a patch proposal for human review.
    pub async fn propose_patch(&self, proposal: PatchProposal) {
        let mut patches = self.pending_patches.write().await;
        patches.push(proposal);
    }

    /// List all pending patch proposals.
    pub async fn list_pending_patches(&self) -> Vec<PatchProposal> {
        let patches = self.pending_patches.read().await;
        patches
            .iter()
            .filter(|p| p.status == PatchStatus::PendingApproval)
            .cloned()
            .collect()
    }

    /// Return true when a tool already has a non-terminal patch proposal.
    pub async fn has_open_proposal_for_tool(&self, tool_name: &str) -> bool {
        let patches = self.pending_patches.read().await;
        patches.iter().any(|p| {
            p.tool_name == tool_name
                && matches!(
                    p.status,
                    PatchStatus::PendingApproval | PatchStatus::Approved
                )
        })
    }

    /// Approve a patch proposal by ID.
    pub async fn approve_patch(&self, patch_id: Uuid) -> bool {
        let mut patches = self.pending_patches.write().await;
        if let Some(patch) = patches.iter_mut().find(|p| p.id == patch_id) {
            patch.status = PatchStatus::Approved;
            true
        } else {
            false
        }
    }

    /// Reject a patch proposal by ID.
    pub async fn reject_patch(&self, patch_id: Uuid) -> bool {
        let mut patches = self.pending_patches.write().await;
        if let Some(patch) = patches.iter_mut().find(|p| p.id == patch_id) {
            patch.status = PatchStatus::Rejected;
            true
        } else {
            false
        }
    }

    /// Get approved patches ready for deployment.
    pub async fn get_approved_patches(&self) -> Vec<PatchProposal> {
        let patches = self.pending_patches.read().await;
        patches
            .iter()
            .filter(|p| p.status == PatchStatus::Approved)
            .cloned()
            .collect()
    }

    /// Mark a patch as deployed.
    pub async fn mark_deployed(&self, patch_id: Uuid) -> bool {
        let mut patches = self.pending_patches.write().await;
        if let Some(patch) = patches.iter_mut().find(|p| p.id == patch_id) {
            patch.status = PatchStatus::Deployed;
            true
        } else {
            false
        }
    }

    /// Mark a patch as failed.
    pub async fn mark_failed(&self, patch_id: Uuid, reason: String) -> bool {
        let mut patches = self.pending_patches.write().await;
        if let Some(patch) = patches.iter_mut().find(|p| p.id == patch_id) {
            patch.status = PatchStatus::Failed(reason);
            true
        } else {
            false
        }
    }

    /// List all patch proposals.
    pub async fn all_patches(&self) -> Vec<PatchProposal> {
        self.pending_patches.read().await.clone()
    }

    /// Get a specific patch proposal by ID.
    pub async fn get_patch(&self, patch_id: Uuid) -> Option<PatchProposal> {
        self.pending_patches
            .read()
            .await
            .iter()
            .find(|p| p.id == patch_id)
            .cloned()
    }
}

impl Default for KernelMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_and_detect_bottleneck() {
        let monitor = KernelMonitor::new().with_threshold(100.0);

        // Record 15 slow executions
        for _ in 0..15 {
            monitor
                .record_execution("slow_tool", Duration::from_millis(500), true)
                .await;
        }

        // Record 15 fast executions
        for _ in 0..15 {
            monitor
                .record_execution("fast_tool", Duration::from_millis(10), true)
                .await;
        }

        let bottlenecks = monitor.detect_bottlenecks().await;
        assert_eq!(bottlenecks.len(), 1);
        assert_eq!(bottlenecks[0].tool_name, "slow_tool");
    }

    #[tokio::test]
    async fn test_patch_approval_workflow() {
        let monitor = KernelMonitor::new();

        let patch = PatchProposal {
            id: Uuid::new_v4(),
            tool_name: "slow_tool".to_string(),
            reason: "Avg latency 500ms".to_string(),
            proposed_rust_code: "fn faster() {}".to_string(),
            expected_speedup: 10.0,
            status: PatchStatus::PendingApproval,
        };
        let patch_id = patch.id;

        monitor.propose_patch(patch).await;
        assert_eq!(monitor.list_pending_patches().await.len(), 1);

        monitor.approve_patch(patch_id).await;
        assert_eq!(monitor.list_pending_patches().await.len(), 0);
        assert_eq!(monitor.get_approved_patches().await.len(), 1);

        monitor.mark_deployed(patch_id).await;
        assert_eq!(monitor.get_approved_patches().await.len(), 0);
    }
}
