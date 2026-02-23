//! Job scheduler for parallel execution.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{RwLock, mpsc, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::task::{Task, TaskContext, TaskOutput};
use crate::agent::worker::{Worker, WorkerDeps};
use crate::config::AgentConfig;
use crate::context::{ContextManager, JobContext, JobState};
use crate::db::Database;
use crate::error::{Error, JobError};
use crate::hooks::HookRegistry;
use crate::llm::LlmProvider;
use crate::safety::SafetyLayer;
use crate::swarm::node::{SwarmHandle, SwarmRemoteResult, SwarmResultRouter};
use crate::tools::ToolRegistry;

// Default remote wait budget for swarm offload before local fallback.
// 300ms is too aggressive for real network + tool execution latency.
const REMOTE_DISTRIBUTE_RETRIES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OffloadDecision {
    LocalOnly,
    RemotePreferred,
}

#[derive(Default)]
struct SchedulerMetrics {
    remote_attempts: AtomicU64,
    remote_successes: AtomicU64,
    remote_failures: AtomicU64,
    remote_timeouts: AtomicU64,
    local_fallbacks: AtomicU64,
    remote_ineligible: AtomicU64,
}

/// Message to send to a worker.
#[derive(Debug)]
pub enum WorkerMessage {
    /// Start working on the job.
    Start,
    /// Stop the job.
    Stop,
    /// Check health.
    Ping,
}

/// Status of a scheduled job.
#[derive(Debug)]
pub struct ScheduledJob {
    pub handle: JoinHandle<()>,
    pub tx: mpsc::Sender<WorkerMessage>,
}

/// Status of a scheduled sub-task.
struct ScheduledSubtask {
    handle: JoinHandle<()>,
}

/// Schedules and manages parallel job execution.
pub struct Scheduler {
    config: AgentConfig,
    context_manager: Arc<ContextManager>,
    llm: Arc<dyn LlmProvider>,
    safety: Arc<SafetyLayer>,
    tools: Arc<ToolRegistry>,
    store: Option<Arc<dyn Database>>,
    hooks: Arc<HookRegistry>,
    swarm_handle: Option<SwarmHandle>,
    swarm_results: Option<SwarmResultRouter>,
    metrics: Arc<SchedulerMetrics>,
    /// Running jobs (main LLM-driven jobs).
    jobs: Arc<RwLock<HashMap<Uuid, ScheduledJob>>>,
    /// Running sub-tasks (tool executions, background tasks).
    subtasks: Arc<RwLock<HashMap<Uuid, ScheduledSubtask>>>,
}

impl Scheduler {
    /// Create a new scheduler.
    pub fn new(
        config: AgentConfig,
        context_manager: Arc<ContextManager>,
        llm: Arc<dyn LlmProvider>,
        safety: Arc<SafetyLayer>,
        tools: Arc<ToolRegistry>,
        store: Option<Arc<dyn Database>>,
        hooks: Arc<HookRegistry>,
        swarm_handle: Option<SwarmHandle>,
        swarm_results: Option<SwarmResultRouter>,
    ) -> Self {
        Self {
            config,
            context_manager,
            llm,
            safety,
            tools,
            store,
            hooks,
            swarm_handle,
            swarm_results,
            metrics: Arc::new(SchedulerMetrics::default()),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            subtasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Schedule a job for execution.
    pub async fn schedule(&self, job_id: Uuid) -> Result<(), JobError> {
        // Hold write lock for the entire check-insert sequence to prevent
        // TOCTOU races where two concurrent calls both pass the checks.
        {
            let mut jobs = self.jobs.write().await;

            if jobs.contains_key(&job_id) {
                return Ok(());
            }

            if jobs.len() >= self.config.max_parallel_jobs {
                return Err(JobError::MaxJobsExceeded {
                    max: self.config.max_parallel_jobs,
                });
            }

            // Transition job to in_progress
            self.context_manager
                .update_context(job_id, |ctx| {
                    ctx.transition_to(
                        JobState::InProgress,
                        Some("Scheduled for execution".to_string()),
                    )
                })
                .await?
                .map_err(|s| JobError::ContextError {
                    id: job_id,
                    reason: s,
                })?;

            // Create worker channel
            let (tx, rx) = mpsc::channel(16);

            // Create worker with shared dependencies
            let deps = WorkerDeps {
                context_manager: self.context_manager.clone(),
                llm: self.llm.clone(),
                safety: self.safety.clone(),
                tools: self.tools.clone(),
                store: self.store.clone(),
                hooks: self.hooks.clone(),
                timeout: self.config.job_timeout,
                use_planning: self.config.use_planning,
            };
            let worker = Worker::new(job_id, deps);

            // Spawn worker task
            let handle = tokio::spawn(async move {
                if let Err(e) = worker.run(rx).await {
                    tracing::error!("Worker for job {} failed: {}", job_id, e);
                }
            });

            // Start the worker
            if let Err(e) = tx.send(WorkerMessage::Start).await {
                tracing::warn!(
                    "Failed to send WorkerMessage::Start for job {}: {}",
                    job_id,
                    e
                );
            }

            // Insert while still holding the write lock
            jobs.insert(job_id, ScheduledJob { handle, tx });
        }

        // Cleanup task for this job to avoid capacity leaks
        let jobs = Arc::clone(&self.jobs);
        tokio::spawn(async move {
            loop {
                let finished = {
                    let jobs_read = jobs.read().await;
                    match jobs_read.get(&job_id) {
                        Some(scheduled) => scheduled.handle.is_finished(),
                        None => true,
                    }
                };

                if finished {
                    jobs.write().await.remove(&job_id);
                    break;
                }

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        tracing::info!("Scheduled job {} for execution", job_id);
        Ok(())
    }

    /// Schedule a sub-task from within a worker.
    ///
    /// Sub-tasks are lightweight tasks that don't go through the full job lifecycle.
    /// They're used for parallel tool execution and background computations.
    ///
    /// Returns a oneshot receiver to get the result.
    pub async fn spawn_subtask(
        &self,
        parent_id: Uuid,
        task: Task,
    ) -> Result<oneshot::Receiver<Result<TaskOutput, Error>>, JobError> {
        let task_id = Uuid::new_v4();
        let (result_tx, result_rx) = oneshot::channel();

        let handle = match task {
            Task::Job { .. } => {
                // Jobs should go through schedule(), not spawn_subtask
                return Err(JobError::ContextError {
                    id: parent_id,
                    reason: "Use schedule() for Job tasks, not spawn_subtask()".to_string(),
                });
            }

            Task::ToolExec {
                parent_id: tool_parent_id,
                tool_name,
                params,
            } => {
                let tools = self.tools.clone();
                let context_manager = self.context_manager.clone();
                let safety = self.safety.clone();
                let swarm_handle = self.swarm_handle.clone();
                let swarm_results = self.swarm_results.clone();
                let task_id_for_remote = task_id;
                let tool_name_for_remote = tool_name.clone();
                let params_for_remote = params.clone();
                let metrics = Arc::clone(&self.metrics);
                let remote_wait_timeout = self.config.swarm_remote_wait_timeout;

                tokio::spawn(async move {
                    let mut used_local_fallback = false;
                    let result = if let (Some(handle), Some(router)) = (swarm_handle, swarm_results)
                    {
                        match decide_offload(
                            &tools,
                            &handle,
                            &tool_name_for_remote,
                            &params_for_remote,
                        )
                        .await
                        {
                            OffloadDecision::LocalOnly => {
                                metrics.remote_ineligible.fetch_add(1, Ordering::Relaxed);
                                used_local_fallback = true;
                                Self::execute_tool_task(
                                    tools,
                                    context_manager,
                                    safety,
                                    tool_parent_id,
                                    &tool_name,
                                    params,
                                )
                                .await
                            }
                            OffloadDecision::RemotePreferred => {
                                metrics.remote_attempts.fetch_add(1, Ordering::Relaxed);
                                let capability =
                                    required_swarm_capability(&tool_name_for_remote).to_string();
                                let assignee_node = match handle.select_peer(&capability).await {
                                    Ok(Some(peer)) => Some(peer),
                                    Ok(None) => {
                                        metrics.remote_ineligible.fetch_add(1, Ordering::Relaxed);
                                        used_local_fallback = true;
                                        tracing::debug!(
                                            tool = %tool_name_for_remote,
                                            capability = %capability,
                                            "No eligible swarm peer selected, falling back to local"
                                        );
                                        None
                                    }
                                    Err(e) => {
                                        metrics.remote_failures.fetch_add(1, Ordering::Relaxed);
                                        used_local_fallback = true;
                                        tracing::warn!(
                                            tool = %tool_name_for_remote,
                                            capability = %capability,
                                            "Swarm peer selection failed, falling back to local: {}",
                                            e
                                        );
                                        None
                                    }
                                };

                                if assignee_node.is_none() {
                                    Self::execute_tool_task(
                                        tools,
                                        context_manager,
                                        safety,
                                        tool_parent_id,
                                        &tool_name,
                                        params,
                                    )
                                    .await
                                } else {
                                match router
                                    .register(task_id_for_remote, remote_wait_timeout)
                                    .await
                                {
                                    Ok(remote_waiter) => {
                                        let mut dispatched = false;
                                        for attempt in 0..=REMOTE_DISTRIBUTE_RETRIES {
                                            let dispatch_res = handle
                                                .distribute_task(
                                                    crate::swarm::protocol::SwarmTask {
                                                        id: task_id_for_remote,
                                                        job_id: tool_parent_id,
                                                        tool_name: tool_name_for_remote.clone(),
                                                        params: params_for_remote.clone(),
                                                        priority: 5,
                                                        attempt: attempt as u8,
                                                        deadline_ms: None,
                                                        origin_node: None,
                                                        assignee_node: assignee_node.clone(),
                                                    },
                                                )
                                                .await;
                                            if dispatch_res.is_ok() {
                                                dispatched = true;
                                                break;
                                            }
                                        }

                                        if dispatched {
                                            match tokio::time::timeout(
                                                remote_wait_timeout,
                                                remote_waiter,
                                            )
                                            .await
                                            {
                                                Ok(Ok(SwarmRemoteResult {
                                                    success: true,
                                                    output,
                                                    duration_ms,
                                                })) => {
                                                    metrics
                                                        .remote_successes
                                                        .fetch_add(1, Ordering::Relaxed);
                                                    let output_json = serde_json::from_str::<
                                                        serde_json::Value,
                                                    >(
                                                        &output
                                                    )
                                                    .unwrap_or_else(
                                                        |_| serde_json::json!({ "output": output }),
                                                    );
                                                    Ok(TaskOutput::new(
                                                        output_json,
                                                        Duration::from_millis(duration_ms),
                                                    ))
                                                }
                                                Ok(Ok(SwarmRemoteResult {
                                                    success: false,
                                                    output,
                                                    ..
                                                })) => {
                                                    metrics
                                                        .remote_failures
                                                        .fetch_add(1, Ordering::Relaxed);
                                                    used_local_fallback = true;
                                                    tracing::warn!(
                                                        tool = %tool_name_for_remote,
                                                        "Remote swarm execution failed, falling back to local: {}",
                                                        output
                                                    );
                                                    Self::execute_tool_task(
                                                        tools,
                                                        context_manager,
                                                        safety,
                                                        tool_parent_id,
                                                        &tool_name,
                                                        params,
                                                    )
                                                    .await
                                                }
                                                _ => {
                                                    metrics
                                                        .remote_timeouts
                                                        .fetch_add(1, Ordering::Relaxed);
                                                    used_local_fallback = true;
                                                    Self::execute_tool_task(
                                                        tools,
                                                        context_manager,
                                                        safety,
                                                        tool_parent_id,
                                                        &tool_name,
                                                        params,
                                                    )
                                                    .await
                                                }
                                            }
                                        } else {
                                            metrics.remote_failures.fetch_add(1, Ordering::Relaxed);
                                            used_local_fallback = true;
                                            Self::execute_tool_task(
                                                tools,
                                                context_manager,
                                                safety,
                                                tool_parent_id,
                                                &tool_name,
                                                params,
                                            )
                                            .await
                                        }
                                    }
                                    Err(_) => {
                                        metrics.remote_ineligible.fetch_add(1, Ordering::Relaxed);
                                        used_local_fallback = true;
                                        Self::execute_tool_task(
                                            tools,
                                            context_manager,
                                            safety,
                                            tool_parent_id,
                                            &tool_name,
                                            params,
                                        )
                                        .await
                                    }
                                }
                                }
                            }
                        }
                    } else {
                        used_local_fallback = true;
                        Self::execute_tool_task(
                            tools,
                            context_manager,
                            safety,
                            tool_parent_id,
                            &tool_name,
                            params,
                        )
                        .await
                    };
                    if used_local_fallback {
                        metrics.local_fallbacks.fetch_add(1, Ordering::Relaxed);
                    }

                    // Send result (ignore if receiver dropped)
                    let _ = result_tx.send(result);
                })
            }

            Task::Background { id: _, handler } => {
                let ctx = TaskContext::new(task_id).with_parent(parent_id);

                tokio::spawn(async move {
                    let result = handler.run(ctx).await;
                    let _ = result_tx.send(result);
                })
            }
        };

        // Track the subtask
        self.subtasks
            .write()
            .await
            .insert(task_id, ScheduledSubtask { handle });

        // Cleanup task for subtask tracking
        let subtasks = Arc::clone(&self.subtasks);
        tokio::spawn(async move {
            loop {
                let finished = {
                    let subtasks_read = subtasks.read().await;
                    match subtasks_read.get(&task_id) {
                        Some(scheduled) => scheduled.handle.is_finished(),
                        None => true,
                    }
                };

                if finished {
                    subtasks.write().await.remove(&task_id);
                    break;
                }

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        tracing::debug!(
            parent_id = %parent_id,
            task_id = %task_id,
            "Spawned subtask"
        );

        Ok(result_rx)
    }

    /// Schedule multiple tasks in parallel and wait for all to complete.
    ///
    /// Returns results in the same order as the input tasks.
    pub async fn spawn_batch(
        &self,
        parent_id: Uuid,
        tasks: Vec<Task>,
    ) -> Vec<Result<TaskOutput, Error>> {
        if tasks.is_empty() {
            return Vec::new();
        }

        let mut receivers = Vec::with_capacity(tasks.len());

        // Spawn all tasks
        for task in tasks {
            match self.spawn_subtask(parent_id, task).await {
                Ok(rx) => receivers.push(Some(rx)),
                Err(e) => {
                    // Store the error directly
                    receivers.push(None);
                    tracing::warn!(
                        parent_id = %parent_id,
                        error = %e,
                        "Failed to spawn subtask in batch"
                    );
                }
            }
        }

        // Collect results
        let mut results = Vec::with_capacity(receivers.len());
        for rx in receivers {
            let result = match rx {
                Some(receiver) => match receiver.await {
                    Ok(task_result) => task_result,
                    Err(_) => Err(Error::Job(JobError::ContextError {
                        id: parent_id,
                        reason: "Subtask channel closed unexpectedly".to_string(),
                    })),
                },
                None => Err(Error::Job(JobError::ContextError {
                    id: parent_id,
                    reason: "Subtask failed to spawn".to_string(),
                })),
            };
            results.push(result);
        }

        results
    }

    /// Execute a single tool as a subtask.
    async fn execute_tool_task(
        tools: Arc<ToolRegistry>,
        context_manager: Arc<ContextManager>,
        safety: Arc<SafetyLayer>,
        job_id: Uuid,
        tool_name: &str,
        params: serde_json::Value,
    ) -> Result<TaskOutput, Error> {
        let start = std::time::Instant::now();

        // Get the tool
        let tool = tools.get(tool_name).await.ok_or_else(|| {
            Error::Tool(crate::error::ToolError::NotFound {
                name: tool_name.to_string(),
            })
        })?;

        // Get job context
        let job_ctx: JobContext = context_manager.get_context(job_id).await?;
        if job_ctx.state == JobState::Cancelled {
            return Err(crate::error::ToolError::ExecutionFailed {
                name: tool_name.to_string(),
                reason: "Job is cancelled".to_string(),
            }
            .into());
        }

        if tool.requires_approval() {
            return Err(crate::error::ToolError::AuthRequired {
                name: tool_name.to_string(),
            }
            .into());
        }

        // Validate tool parameters
        let validation = safety.validator().validate_tool_params(&params);
        if !validation.is_valid {
            let details = validation
                .errors
                .iter()
                .map(|e| format!("{}: {}", e.field, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(crate::error::ToolError::InvalidParameters {
                name: tool_name.to_string(),
                reason: format!("Invalid tool parameters: {}", details),
            }
            .into());
        }

        // Execute with per-tool timeout
        let tool_timeout = tool.execution_timeout();
        let result =
            tokio::time::timeout(tool_timeout, async { tool.execute(params, &job_ctx).await })
                .await
                .map_err(|_| {
                    Error::Tool(crate::error::ToolError::Timeout {
                        name: tool_name.to_string(),
                        timeout: tool_timeout,
                    })
                })?
                .map_err(|e| {
                    Error::Tool(crate::error::ToolError::ExecutionFailed {
                        name: tool_name.to_string(),
                        reason: e.to_string(),
                    })
                })?;

        Ok(TaskOutput::new(result.result, start.elapsed()))
    }

    /// Stop a running job.
    pub async fn stop(&self, job_id: Uuid) -> Result<(), JobError> {
        let mut jobs = self.jobs.write().await;

        if let Some(scheduled) = jobs.remove(&job_id) {
            // Send stop signal
            let _ = scheduled.tx.send(WorkerMessage::Stop).await;

            // Give it a moment to clean up
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Abort if still running
            if !scheduled.handle.is_finished() {
                scheduled.handle.abort();
            }

            // Update job state
            self.context_manager
                .update_context(job_id, |ctx| {
                    let _ = ctx.transition_to(
                        JobState::Cancelled,
                        Some("Stopped by scheduler".to_string()),
                    );
                })
                .await?;

            // Persist cancellation (fire-and-forget)
            if let Some(ref store) = self.store {
                let store = store.clone();
                tokio::spawn(async move {
                    if let Err(e) = store
                        .update_job_status(
                            job_id,
                            JobState::Cancelled,
                            Some("Stopped by scheduler"),
                        )
                        .await
                    {
                        tracing::warn!("Failed to persist cancellation for job {}: {}", job_id, e);
                    }
                });
            }

            tracing::info!("Stopped job {}", job_id);
        }

        Ok(())
    }

    /// Check if a job is running.
    pub async fn is_running(&self, job_id: Uuid) -> bool {
        self.jobs.read().await.contains_key(&job_id)
    }

    /// Get count of running jobs.
    pub async fn running_count(&self) -> usize {
        self.jobs.read().await.len()
    }

    /// Get count of running subtasks.
    pub async fn subtask_count(&self) -> usize {
        self.subtasks.read().await.len()
    }

    /// Get all running job IDs.
    pub async fn running_jobs(&self) -> Vec<Uuid> {
        self.jobs.read().await.keys().cloned().collect()
    }

    /// Clean up finished jobs and subtasks.
    pub async fn cleanup_finished(&self) {
        // Clean up jobs
        {
            let mut jobs = self.jobs.write().await;
            let mut finished = Vec::new();

            for (id, scheduled) in jobs.iter() {
                if scheduled.handle.is_finished() {
                    finished.push(*id);
                }
            }

            for id in finished {
                jobs.remove(&id);
                tracing::debug!("Cleaned up finished job {}", id);
            }
        }

        // Clean up subtasks
        {
            let mut subtasks = self.subtasks.write().await;
            let mut finished = Vec::new();

            for (id, scheduled) in subtasks.iter() {
                if scheduled.handle.is_finished() {
                    finished.push(*id);
                }
            }

            for id in finished {
                subtasks.remove(&id);
                tracing::trace!("Cleaned up finished subtask {}", id);
            }
        }
    }

    /// Stop all jobs.
    pub async fn stop_all(&self) {
        let job_ids: Vec<Uuid> = self.jobs.read().await.keys().cloned().collect();

        for job_id in job_ids {
            let _ = self.stop(job_id).await;
        }

        // Abort all subtasks
        let mut subtasks = self.subtasks.write().await;
        for (_, scheduled) in subtasks.drain() {
            scheduled.handle.abort();
        }
    }

    /// Get access to the tools registry.
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    /// Get access to the context manager.
    pub fn context_manager(&self) -> &Arc<ContextManager> {
        &self.context_manager
    }
}

fn required_swarm_capability(tool_name: &str) -> &'static str {
    match tool_name {
        "shell" => "shell",
        "memory_search" | "memory_graph" => "memory_search",
        _ => "tool_exec",
    }
}

async fn decide_offload(
    tools: &Arc<ToolRegistry>,
    swarm: &SwarmHandle,
    tool_name: &str,
    params: &serde_json::Value,
) -> OffloadDecision {
    let Some(tool) = tools.get(tool_name).await else {
        return OffloadDecision::LocalOnly;
    };

    if tool.requires_approval() || tool.requires_approval_for(params) {
        return OffloadDecision::LocalOnly;
    }

    let capability = required_swarm_capability(tool_name);
    match swarm.has_capability(capability).await {
        Ok(true) => OffloadDecision::RemotePreferred,
        Ok(false) | Err(_) => OffloadDecision::LocalOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::required_swarm_capability;

    #[test]
    fn test_scheduler_creation() {
        // Would need to mock dependencies for proper testing
    }

    #[test]
    fn test_required_swarm_capability_mapping() {
        assert_eq!(required_swarm_capability("shell"), "shell");
        assert_eq!(required_swarm_capability("memory_search"), "memory_search");
        assert_eq!(required_swarm_capability("memory_graph"), "memory_search");
        assert_eq!(required_swarm_capability("list_jobs"), "tool_exec");
    }

    #[tokio::test]
    async fn test_spawn_batch_empty() {
        // This test would need mock dependencies.
        // For now just verify the empty case doesn't panic.
    }
}
