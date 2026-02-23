//! Hive mesh node using libp2p.
//!
//! Each IronClaw instance runs a `SwarmNode` that:
//! 1. Discovers peers on the local network via mDNS
//! 2. Communicates task assignments/results via GossipSub
//! 3. Maintains a DHT via Kademlia for WAN discovery
//! 4. Broadcasts heartbeats so peers know each node's capacity

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;

use libp2p::{
    Multiaddr, PeerId, SwarmBuilder, gossipsub, identify, kad, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

use super::protocol::{SwarmMessage, SwarmTask, TOPIC_HEARTBEAT, TOPIC_RESULTS, TOPIC_TASKS};

const SEEN_TASK_TTL: Duration = Duration::from_secs(300);
const SEEN_TASK_MAX: usize = 4096;

/// Combined libp2p behaviour for the Hive mesh.
#[derive(NetworkBehaviour)]
pub struct HiveBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub identify: identify::Behaviour,
}

/// Handle for sending commands to the running swarm node.
#[derive(Clone)]
pub struct SwarmHandle {
    cmd_tx: mpsc::Sender<SwarmCommand>,
}

/// Remote task completion payload delivered to local waiters.
#[derive(Debug)]
pub struct SwarmRemoteResult {
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
}

/// Shared map used to route remote task results back to awaiting callers.
#[derive(Clone, Default)]
pub struct SwarmResultRouter {
    pending: Arc<Mutex<HashMap<Uuid, PendingWaiter>>>,
}

#[derive(Debug)]
struct PendingWaiter {
    sender: oneshot::Sender<SwarmRemoteResult>,
    expires_at: Instant,
}

#[derive(Debug)]
pub enum SwarmRouterError {
    Saturated { max_pending: usize },
}

impl SwarmResultRouter {
    pub const MAX_PENDING_WAITERS: usize = 4096;

    /// Register interest in a task result and return a receiver for it.
    pub async fn register(
        &self,
        task_id: Uuid,
        timeout: Duration,
    ) -> Result<oneshot::Receiver<SwarmRemoteResult>, SwarmRouterError> {
        let _ = self.cleanup_expired().await;
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending.lock().await;
        if pending.len() >= Self::MAX_PENDING_WAITERS {
            return Err(SwarmRouterError::Saturated {
                max_pending: Self::MAX_PENDING_WAITERS,
            });
        }
        pending.insert(
            task_id,
            PendingWaiter {
                sender: tx,
                expires_at: Instant::now() + timeout,
            },
        );
        Ok(rx)
    }

    /// Resolve a task result for any local waiter.
    pub async fn resolve(&self, task_id: Uuid, result: SwarmRemoteResult) -> bool {
        if let Some(waiter) = self.pending.lock().await.remove(&task_id) {
            let _ = waiter.sender.send(result);
            true
        } else {
            false
        }
    }

    /// Remove expired waiters and return number removed.
    pub async fn cleanup_expired(&self) -> usize {
        let now = Instant::now();
        let mut pending = self.pending.lock().await;
        let before = pending.len();
        pending.retain(|_, waiter| waiter.expires_at > now);
        before.saturating_sub(pending.len())
    }

    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

/// Commands that external code can send to the swarm event loop.
pub enum SwarmCommand {
    /// Distribute a task to the mesh.
    DistributeTask(SwarmTask),
    /// Broadcast this node's capacity.
    BroadcastHeartbeat { available_slots: u32 },
    /// Publish a task result.
    PublishResult {
        task_id: Uuid,
        success: bool,
        output: String,
        duration_ms: u64,
    },
    /// Query whether any known peer currently advertises a capability.
    HasCapability {
        capability: String,
        respond_to: oneshot::Sender<bool>,
    },
    /// Select a specific peer advertising a capability (best-effort).
    SelectPeer {
        capability: String,
        respond_to: oneshot::Sender<Option<String>>,
    },
    /// Shut down the swarm.
    Shutdown,
}

/// Events emitted by the swarm to the local application.
pub enum SwarmEvent2 {
    /// A peer assigned us a task.
    IncomingTask(SwarmTask),
    /// A remote task completed.
    TaskCompleted {
        task_id: Uuid,
        success: bool,
        output: String,
        duration_ms: u64,
    },
    /// A new peer was discovered.
    PeerDiscovered(PeerId),
    /// A peer disconnected.
    PeerLost(PeerId),
}

/// Configuration for the swarm node.
pub struct SwarmConfig {
    /// Port to listen on (0 = random).
    pub listen_port: u16,
    /// Heartbeat interval.
    pub heartbeat_interval: Duration,
    /// Maximum available execution slots to advertise.
    pub max_slots: u32,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            listen_port: 0,
            heartbeat_interval: Duration::from_secs(15),
            max_slots: 4,
        }
    }
}

/// The Hive swarm node.
pub struct SwarmNode {
    config: SwarmConfig,
}

#[derive(Debug, Clone)]
struct PeerState {
    available_slots: u32,
    capabilities: Vec<String>,
}

impl SwarmNode {
    pub fn new(config: SwarmConfig) -> Self {
        Self { config }
    }

    /// Start the swarm node event loop.
    ///
    /// Returns a handle for sending commands and a receiver for swarm events.
    pub async fn start(
        self,
    ) -> Result<(SwarmHandle, mpsc::Receiver<SwarmEvent2>), Box<dyn std::error::Error + Send + Sync>>
    {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SwarmCommand>(256);
        let (event_tx, event_rx) = mpsc::channel::<SwarmEvent2>(256);

        let mut swarm = SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                // GossipSub
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(10))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .build()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )?;

                // mDNS for local peer discovery
                let mdns = mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?;

                // Kademlia DHT
                let store = kad::store::MemoryStore::new(key.public().to_peer_id());
                let kademlia = kad::Behaviour::new(key.public().to_peer_id(), store);

                // Identify
                let identify = identify::Behaviour::new(identify::Config::new(
                    "/ironclaw/hive/1.0.0".to_string(),
                    key.public(),
                ));

                Ok(HiveBehaviour {
                    gossipsub,
                    mdns,
                    kademlia,
                    identify,
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        // Subscribe to topics
        let heartbeat_topic = gossipsub::IdentTopic::new(TOPIC_HEARTBEAT);
        let tasks_topic = gossipsub::IdentTopic::new(TOPIC_TASKS);
        let results_topic = gossipsub::IdentTopic::new(TOPIC_RESULTS);

        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&heartbeat_topic)
            .map_err(|e| format!("Failed to subscribe to heartbeat: {:?}", e))?;
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&tasks_topic)
            .map_err(|e| format!("Failed to subscribe to tasks: {:?}", e))?;
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&results_topic)
            .map_err(|e| format!("Failed to subscribe to results: {:?}", e))?;

        // Listen
        let listen_addr: Multiaddr =
            format!("/ip4/0.0.0.0/tcp/{}", self.config.listen_port).parse()?;
        swarm.listen_on(listen_addr)?;

        let node_id = swarm.local_peer_id().to_string();
        tracing::info!("Hive swarm node started: {}", node_id);

        let heartbeat_interval = self.config.heartbeat_interval;
        let max_slots = self.config.max_slots;

        // Spawn the event loop
        tokio::spawn(async move {
            let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
            let mut peers: HashMap<PeerId, PeerState> = HashMap::new();
            let mut seen_tasks: HashMap<Uuid, Instant> = HashMap::new();
            let advertised_capabilities = vec![
                "tool_exec".to_string(),
                "shell".to_string(),
                "memory_search".to_string(),
                "wasm".to_string(),
            ];

            loop {
                tokio::select! {
                    // Process swarm events
                    event = swarm.select_next_some() => {
                        match event {
                            SwarmEvent::Behaviour(HiveBehaviourEvent::Gossipsub(
                                gossipsub::Event::Message { message, .. },
                            )) => {
                                if let Ok(msg) = serde_json::from_slice::<SwarmMessage>(&message.data) {
                                    match msg {
                                        SwarmMessage::TaskAssignment(task) => {
                                            if should_accept_incoming_task(
                                                &node_id,
                                                &task,
                                                &mut seen_tasks,
                                                Instant::now(),
                                            ) {
                                                let _ = event_tx.send(SwarmEvent2::IncomingTask(task)).await;
                                            }
                                        }
                                        SwarmMessage::TaskResult { task_id, success, output, duration_ms } => {
                                            let _ = event_tx.send(SwarmEvent2::TaskCompleted {
                                                task_id, success, output, duration_ms,
                                            }).await;
                                        }
                                        SwarmMessage::Heartbeat { node_id: _, available_slots, capabilities } => {
                                            if let Some(source) = message.source {
                                                peers.insert(
                                                    source,
                                                    PeerState {
                                                        available_slots,
                                                        capabilities,
                                                    },
                                                );
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            SwarmEvent::Behaviour(HiveBehaviourEvent::Mdns(
                                mdns::Event::Discovered(list),
                            )) => {
                                for (peer_id, multiaddr) in list {
                                    tracing::info!("mDNS discovered peer: {} at {}", peer_id, multiaddr);
                                    swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                                    swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
                                    let _ = event_tx.send(SwarmEvent2::PeerDiscovered(peer_id)).await;
                                }
                            }
                            SwarmEvent::Behaviour(HiveBehaviourEvent::Mdns(
                                mdns::Event::Expired(list),
                            )) => {
                                for (peer_id, _) in list {
                                    tracing::info!("mDNS peer expired: {}", peer_id);
                                    swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                                    peers.remove(&peer_id);
                                    let _ = event_tx.send(SwarmEvent2::PeerLost(peer_id)).await;
                                }
                            }
                            _ => {}
                        }
                    }

                    // Process commands from application
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(SwarmCommand::DistributeTask(task)) => {
                                let msg = SwarmMessage::TaskAssignment(task);
                                if let Ok(data) = serde_json::to_vec(&msg) {
                                    let topic = gossipsub::IdentTopic::new(TOPIC_TASKS);
                                    let _ = swarm.behaviour_mut().gossipsub.publish(topic, data);
                                }
                            }
                            Some(SwarmCommand::PublishResult { task_id, success, output, duration_ms }) => {
                                let msg = SwarmMessage::TaskResult { task_id, success, output, duration_ms };
                                if let Ok(data) = serde_json::to_vec(&msg) {
                                    let topic = gossipsub::IdentTopic::new(TOPIC_RESULTS);
                                    let _ = swarm.behaviour_mut().gossipsub.publish(topic, data);
                                }
                            }
                            Some(SwarmCommand::BroadcastHeartbeat { available_slots }) => {
                                let msg = SwarmMessage::Heartbeat {
                                    node_id: swarm.local_peer_id().to_string(),
                                    available_slots,
                                    capabilities: advertised_capabilities.clone(),
                                };
                                if let Ok(data) = serde_json::to_vec(&msg) {
                                    let topic = gossipsub::IdentTopic::new(TOPIC_HEARTBEAT);
                                    let _ = swarm.behaviour_mut().gossipsub.publish(topic, data);
                                }
                            }
                            Some(SwarmCommand::HasCapability { capability, respond_to }) => {
                                let has_peer = peers.values().any(|peer| {
                                    peer.available_slots > 0
                                        && peer.capabilities.iter().any(|c| c == &capability)
                                });
                                let _ = respond_to.send(has_peer);
                            }
                            Some(SwarmCommand::SelectPeer { capability, respond_to }) => {
                                let selected = peers
                                    .iter()
                                    .filter(|(_, peer)| {
                                        peer.available_slots > 0
                                            && peer.capabilities.iter().any(|c| c == &capability)
                                    })
                                    .max_by(|(peer_a, state_a), (peer_b, state_b)| {
                                        state_a
                                            .available_slots
                                            .cmp(&state_b.available_slots)
                                            .then_with(|| peer_b.to_string().cmp(&peer_a.to_string()))
                                    })
                                    .map(|(peer_id, _)| peer_id.to_string());
                                let _ = respond_to.send(selected);
                            }
                            Some(SwarmCommand::Shutdown) | None => {
                                tracing::info!("Hive swarm node shutting down");
                                break;
                            }
                        }
                    }

                    // Periodic heartbeat
                    _ = heartbeat_timer.tick() => {
                        let msg = SwarmMessage::Heartbeat {
                            node_id: swarm.local_peer_id().to_string(),
                            available_slots: max_slots,
                            capabilities: advertised_capabilities.clone(),
                        };
                        if let Ok(data) = serde_json::to_vec(&msg) {
                            let topic = gossipsub::IdentTopic::new(TOPIC_HEARTBEAT);
                            let _ = swarm.behaviour_mut().gossipsub.publish(topic, data);
                        }
                    }
                }
            }
        });

        Ok((SwarmHandle { cmd_tx }, event_rx))
    }
}

impl SwarmHandle {
    /// Distribute a task to the mesh for remote execution.
    pub async fn distribute_task(&self, task: SwarmTask) -> Result<(), String> {
        self.cmd_tx
            .send(SwarmCommand::DistributeTask(task))
            .await
            .map_err(|e| format!("Swarm channel closed: {}", e))
    }

    /// Publish a completed task result to the mesh.
    pub async fn publish_result(
        &self,
        task_id: Uuid,
        success: bool,
        output: String,
        duration_ms: u64,
    ) -> Result<(), String> {
        self.cmd_tx
            .send(SwarmCommand::PublishResult {
                task_id,
                success,
                output,
                duration_ms,
            })
            .await
            .map_err(|e| format!("Swarm channel closed: {}", e))
    }

    /// Check if any currently known peer advertises a given capability.
    pub async fn has_capability(&self, capability: &str) -> Result<bool, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(SwarmCommand::HasCapability {
                capability: capability.to_string(),
                respond_to: tx,
            })
            .await
            .map_err(|e| format!("Swarm channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Swarm capability query failed: {}", e))
    }

    /// Select a specific peer that currently advertises the requested capability.
    pub async fn select_peer(&self, capability: &str) -> Result<Option<String>, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(SwarmCommand::SelectPeer {
                capability: capability.to_string(),
                respond_to: tx,
            })
            .await
            .map_err(|e| format!("Swarm channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Swarm peer selection query failed: {}", e))
    }

    /// Shut down the swarm node.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.cmd_tx
            .send(SwarmCommand::Shutdown)
            .await
            .map_err(|e| format!("Swarm channel closed: {}", e))
    }
}

fn should_accept_incoming_task(
    node_id: &str,
    task: &SwarmTask,
    seen_tasks: &mut HashMap<Uuid, Instant>,
    now: Instant,
) -> bool {
    seen_tasks.retain(|_, seen_at| now.saturating_duration_since(*seen_at) < SEEN_TASK_TTL);
    if seen_tasks.len() > SEEN_TASK_MAX {
        // Best-effort trim of oldest entries to bound memory.
        let mut entries: Vec<(Uuid, Instant)> = seen_tasks.iter().map(|(k, v)| (*k, *v)).collect();
        entries.sort_by_key(|(_, t)| *t);
        let drop_count = entries.len().saturating_sub(SEEN_TASK_MAX);
        for (task_id, _) in entries.into_iter().take(drop_count) {
            seen_tasks.remove(&task_id);
        }
    }

    if let Some(assignee) = &task.assignee_node
        && assignee != node_id
    {
        return false;
    }

    if seen_tasks.contains_key(&task.id) {
        return false;
    }
    seen_tasks.insert(task.id, now);
    true
}

#[cfg(test)]
mod tests {
    use super::{SwarmRemoteResult, SwarmResultRouter, should_accept_incoming_task};
    use crate::swarm::protocol::SwarmTask;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    #[tokio::test]
    async fn router_register_resolve_roundtrip() {
        let router = SwarmResultRouter::default();
        let task_id = Uuid::new_v4();
        let rx = router
            .register(task_id, Duration::from_secs(1))
            .await
            .expect("register should succeed");
        let resolved = router
            .resolve(
                task_id,
                SwarmRemoteResult {
                    success: true,
                    output: "ok".to_string(),
                    duration_ms: 12,
                },
            )
            .await;
        assert!(resolved);
        let msg = rx.await.expect("receiver should get result");
        assert!(msg.success);
        assert_eq!(msg.output, "ok");
    }

    #[tokio::test]
    async fn router_cleanup_expired_waiters() {
        let router = SwarmResultRouter::default();
        let _ = router
            .register(Uuid::new_v4(), Duration::from_millis(10))
            .await
            .expect("register should succeed");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let removed = router.cleanup_expired().await;
        assert_eq!(removed, 1);
        assert_eq!(router.pending_count().await, 0);
    }

    #[test]
    fn incoming_task_rejects_non_assignee() {
        let mut seen = HashMap::new();
        let task = SwarmTask {
            id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            tool_name: "echo".to_string(),
            params: json!({"x":1}),
            priority: 1,
            attempt: 0,
            deadline_ms: None,
            origin_node: Some("origin".to_string()),
            assignee_node: Some("peer-b".to_string()),
        };
        assert!(!should_accept_incoming_task(
            "peer-a",
            &task,
            &mut seen,
            Instant::now()
        ));
        assert!(seen.is_empty());
    }

    #[test]
    fn incoming_task_rejects_duplicates() {
        let mut seen = HashMap::new();
        let task = SwarmTask {
            id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            tool_name: "echo".to_string(),
            params: json!({}),
            priority: 1,
            attempt: 0,
            deadline_ms: None,
            origin_node: Some("origin".to_string()),
            assignee_node: None,
        };
        let now = Instant::now();
        assert!(should_accept_incoming_task("peer-a", &task, &mut seen, now));
        assert!(!should_accept_incoming_task("peer-a", &task, &mut seen, now));
    }

    #[test]
    fn incoming_task_allows_after_ttl_expiry() {
        let mut seen = HashMap::new();
        let task = SwarmTask {
            id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            tool_name: "echo".to_string(),
            params: json!({}),
            priority: 1,
            attempt: 0,
            deadline_ms: None,
            origin_node: None,
            assignee_node: None,
        };
        let t0 = Instant::now();
        assert!(should_accept_incoming_task("peer-a", &task, &mut seen, t0));
        let t1 = t0 + Duration::from_secs(301);
        assert!(should_accept_incoming_task("peer-a", &task, &mut seen, t1));
    }
}
