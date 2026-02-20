//! Task distribution protocol messages.
//!
//! Defines the wire format for inter-node communication over GossipSub.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A task that can be distributed across the swarm mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmTask {
    /// Unique task identifier.
    pub id: Uuid,
    /// The parent job this task belongs to.
    pub job_id: Uuid,
    /// Which tool to execute.
    pub tool_name: String,
    /// Tool input parameters (JSON).
    pub params: serde_json::Value,
    /// Priority (higher = more urgent).
    pub priority: u8,
}

/// Messages exchanged between swarm peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SwarmMessage {
    /// Announce this node's availability and capacity.
    Heartbeat {
        node_id: String,
        available_slots: u32,
        capabilities: Vec<String>,
    },
    /// Request a peer to execute a task.
    TaskAssignment(SwarmTask),
    /// Report task completion.
    TaskResult {
        task_id: Uuid,
        success: bool,
        output: String,
        duration_ms: u64,
    },
    /// Request task status.
    TaskQuery { task_id: Uuid },
}

/// GossipSub topic names.
pub const TOPIC_HEARTBEAT: &str = "ironclaw/hive/heartbeat";
pub const TOPIC_TASKS: &str = "ironclaw/hive/tasks";
pub const TOPIC_RESULTS: &str = "ironclaw/hive/results";
