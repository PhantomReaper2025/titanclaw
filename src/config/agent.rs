use std::time::Duration;

use crate::config::helpers::optional_env;
use crate::error::ConfigError;
use crate::settings::Settings;

/// Agent behavior configuration.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub max_parallel_jobs: usize,
    pub job_timeout: Duration,
    pub stuck_threshold: Duration,
    pub repair_check_interval: Duration,
    pub max_repair_attempts: u32,
    /// Whether to use planning before tool execution.
    pub use_planning: bool,
    /// Session idle timeout. Sessions inactive longer than this are pruned.
    pub session_idle_timeout: Duration,
    /// Remote swarm wait timeout before local fallback.
    pub swarm_remote_wait_timeout: Duration,
    /// Allow chat to use filesystem/shell tools directly (bypass sandbox).
    pub allow_local_tools: bool,
    /// Maximum daily LLM spend in cents (e.g. 10000 = $100). None = unlimited.
    pub max_cost_per_day_cents: Option<u64>,
    /// Maximum LLM/tool actions per hour. None = unlimited.
    pub max_actions_per_hour: Option<u64>,
    /// Enable token-to-tool piped execution path.
    pub enable_piped_tool_execution: bool,
    /// Enable speculative shadow workers.
    pub shadow_workers_enabled: bool,
    /// Number of follow-up prompts to precompute per turn.
    pub shadow_max_predictions: usize,
    /// Speculative response cache TTL.
    pub shadow_cache_ttl: Duration,
    /// Max concurrent speculative jobs.
    pub shadow_max_parallel: usize,
    /// Minimum user-input length before shadow speculation.
    pub shadow_min_input_chars: usize,
    /// Enable kernel monitor orchestration loop.
    pub kernel_monitor_enabled: bool,
    /// Kernel monitor analysis cadence.
    pub kernel_monitor_interval: Duration,
    /// Slow-tool threshold used by kernel monitor.
    pub kernel_slow_threshold_ms: f64,
    /// Auto-approve generated kernel patch proposals.
    pub kernel_auto_approve_patches: bool,
    /// Auto-deploy approved kernel patch proposals.
    pub kernel_auto_deploy_patches: bool,
    /// Enable background profile/identity synthesis into workspace managed sections.
    pub profile_synthesis_enabled: bool,
    /// Debounce interval for profile synthesis flushes.
    pub profile_synthesis_debounce: Duration,
    /// Max successful turns batched per synthesis run.
    pub profile_synthesis_max_batch_turns: usize,
    /// Minimum input length before synthesis is considered.
    pub profile_synthesis_min_chars: usize,
    /// Enable LLM-assisted merge of synthesized profile sections.
    pub profile_synthesis_llm_enabled: bool,
}

impl AgentConfig {
    pub(crate) fn resolve(settings: &Settings) -> Result<Self, ConfigError> {
        Ok(Self {
            name: optional_env("AGENT_NAME")?.unwrap_or_else(|| settings.agent.name.clone()),
            max_parallel_jobs: optional_env("AGENT_MAX_PARALLEL_JOBS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "AGENT_MAX_PARALLEL_JOBS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.agent.max_parallel_jobs as usize),
            job_timeout: Duration::from_secs(
                optional_env("AGENT_JOB_TIMEOUT_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "AGENT_JOB_TIMEOUT_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.agent.job_timeout_secs),
            ),
            stuck_threshold: Duration::from_secs(
                optional_env("AGENT_STUCK_THRESHOLD_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "AGENT_STUCK_THRESHOLD_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.agent.stuck_threshold_secs),
            ),
            repair_check_interval: Duration::from_secs(
                optional_env("SELF_REPAIR_CHECK_INTERVAL_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "SELF_REPAIR_CHECK_INTERVAL_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.agent.repair_check_interval_secs),
            ),
            max_repair_attempts: optional_env("SELF_REPAIR_MAX_ATTEMPTS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SELF_REPAIR_MAX_ATTEMPTS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.agent.max_repair_attempts),
            use_planning: optional_env("AGENT_USE_PLANNING")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "AGENT_USE_PLANNING".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.use_planning),
            session_idle_timeout: Duration::from_secs(
                optional_env("SESSION_IDLE_TIMEOUT_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "SESSION_IDLE_TIMEOUT_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.agent.session_idle_timeout_secs),
            ),
            swarm_remote_wait_timeout: Duration::from_millis(
                optional_env("SWARM_REMOTE_WAIT_TIMEOUT_MS")?
                    .map(|s| s.parse::<u64>())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "SWARM_REMOTE_WAIT_TIMEOUT_MS".to_string(),
                        message: format!("must be a positive integer milliseconds value: {e}"),
                    })?
                    .unwrap_or(settings.agent.swarm_remote_wait_timeout_ms)
                    .clamp(200, 15_000),
            ),
            allow_local_tools: optional_env("ALLOW_LOCAL_TOOLS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "ALLOW_LOCAL_TOOLS".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(false),
            max_cost_per_day_cents: optional_env("MAX_COST_PER_DAY_CENTS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "MAX_COST_PER_DAY_CENTS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?,
            max_actions_per_hour: optional_env("MAX_ACTIONS_PER_HOUR")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "MAX_ACTIONS_PER_HOUR".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?,
            enable_piped_tool_execution: optional_env("ENABLE_PIPED_TOOL_EXECUTION")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "ENABLE_PIPED_TOOL_EXECUTION".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.enable_piped_tool_execution),
            shadow_workers_enabled: optional_env("SHADOW_WORKERS_ENABLED")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SHADOW_WORKERS_ENABLED".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.shadow_workers_enabled),
            shadow_max_predictions: optional_env("SHADOW_MAX_PREDICTIONS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SHADOW_MAX_PREDICTIONS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.agent.shadow_max_predictions as usize),
            shadow_cache_ttl: Duration::from_secs(
                optional_env("SHADOW_CACHE_TTL_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "SHADOW_CACHE_TTL_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.agent.shadow_cache_ttl_secs),
            ),
            shadow_max_parallel: optional_env("SHADOW_MAX_PARALLEL")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SHADOW_MAX_PARALLEL".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.agent.shadow_max_parallel as usize),
            shadow_min_input_chars: optional_env("SHADOW_MIN_INPUT_CHARS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SHADOW_MIN_INPUT_CHARS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.agent.shadow_min_input_chars as usize),
            kernel_monitor_enabled: optional_env("KERNEL_MONITOR_ENABLED")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "KERNEL_MONITOR_ENABLED".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.kernel_monitor_enabled),
            kernel_monitor_interval: Duration::from_secs(
                optional_env("KERNEL_MONITOR_INTERVAL_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "KERNEL_MONITOR_INTERVAL_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.agent.kernel_monitor_interval_secs),
            ),
            kernel_slow_threshold_ms: optional_env("KERNEL_SLOW_THRESHOLD_MS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "KERNEL_SLOW_THRESHOLD_MS".to_string(),
                    message: format!("must be a positive number: {e}"),
                })?
                .unwrap_or(settings.agent.kernel_slow_threshold_ms),
            kernel_auto_approve_patches: optional_env("KERNEL_AUTO_APPROVE_PATCHES")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "KERNEL_AUTO_APPROVE_PATCHES".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.kernel_auto_approve_patches),
            kernel_auto_deploy_patches: optional_env("KERNEL_AUTO_DEPLOY_PATCHES")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "KERNEL_AUTO_DEPLOY_PATCHES".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.kernel_auto_deploy_patches),
            profile_synthesis_enabled: optional_env("PROFILE_SYNTHESIS_ENABLED")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "PROFILE_SYNTHESIS_ENABLED".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.profile_synthesis_enabled),
            profile_synthesis_debounce: Duration::from_secs(
                optional_env("PROFILE_SYNTHESIS_DEBOUNCE_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "PROFILE_SYNTHESIS_DEBOUNCE_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.agent.profile_synthesis_debounce_secs),
            ),
            profile_synthesis_max_batch_turns: optional_env("PROFILE_SYNTHESIS_MAX_BATCH_TURNS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "PROFILE_SYNTHESIS_MAX_BATCH_TURNS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.agent.profile_synthesis_max_batch_turns as usize),
            profile_synthesis_min_chars: optional_env("PROFILE_SYNTHESIS_MIN_CHARS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "PROFILE_SYNTHESIS_MIN_CHARS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.agent.profile_synthesis_min_chars as usize),
            profile_synthesis_llm_enabled: optional_env("PROFILE_SYNTHESIS_LLM_ENABLED")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "PROFILE_SYNTHESIS_LLM_ENABLED".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.agent.profile_synthesis_llm_enabled),
        })
    }
}
