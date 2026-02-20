use std::time::Duration;

use crate::config::helpers::optional_env;
use crate::error::ConfigError;
use crate::settings::Settings;

/// Distributed swarm mesh configuration.
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    pub enabled: bool,
    pub listen_port: u16,
    pub heartbeat_interval: Duration,
    pub max_slots: u32,
}

impl SwarmConfig {
    pub(crate) fn resolve(settings: &Settings) -> Result<Self, ConfigError> {
        Ok(Self {
            enabled: optional_env("SWARM_ENABLED")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SWARM_ENABLED".to_string(),
                    message: format!("must be 'true' or 'false': {e}"),
                })?
                .unwrap_or(settings.swarm.enabled),
            listen_port: optional_env("SWARM_LISTEN_PORT")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SWARM_LISTEN_PORT".to_string(),
                    message: format!("must be a valid port (0-65535): {e}"),
                })?
                .unwrap_or(settings.swarm.listen_port),
            heartbeat_interval: Duration::from_secs(
                optional_env("SWARM_HEARTBEAT_INTERVAL_SECS")?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e| ConfigError::InvalidValue {
                        key: "SWARM_HEARTBEAT_INTERVAL_SECS".to_string(),
                        message: format!("must be a positive integer: {e}"),
                    })?
                    .unwrap_or(settings.swarm.heartbeat_interval_secs),
            ),
            max_slots: optional_env("SWARM_MAX_SLOTS")?
                .map(|s| s.parse())
                .transpose()
                .map_err(|e| ConfigError::InvalidValue {
                    key: "SWARM_MAX_SLOTS".to_string(),
                    message: format!("must be a positive integer: {e}"),
                })?
                .unwrap_or(settings.swarm.max_slots),
        })
    }
}
