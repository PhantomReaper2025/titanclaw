//! Hive mesh node using libp2p.
//!
//! Each IronClaw instance runs a `SwarmNode` that:
//! 1. Discovers peers on the local network via mDNS
//! 2. Communicates task assignments/results via GossipSub
//! 3. Maintains a DHT via Kademlia for WAN discovery
//! 4. Broadcasts heartbeats so peers know each node's capacity

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;

use libp2p::{
    gossipsub, identify, kad, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, SwarmBuilder,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::protocol::{SwarmMessage, SwarmTask, TOPIC_HEARTBEAT, TOPIC_RESULTS, TOPIC_TASKS};

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
            let mut peers: HashMap<PeerId, u32> = HashMap::new();

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
                                            let _ = event_tx.send(SwarmEvent2::IncomingTask(task)).await;
                                        }
                                        SwarmMessage::TaskResult { task_id, success, output, duration_ms } => {
                                            let _ = event_tx.send(SwarmEvent2::TaskCompleted {
                                                task_id, success, output, duration_ms,
                                            }).await;
                                        }
                                        SwarmMessage::Heartbeat { node_id: _, available_slots, .. } => {
                                            if let Some(source) = message.source {
                                                peers.insert(source, available_slots);
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
                                    capabilities: vec![
                                        "wasm".into(),
                                        "llm".into(),
                                        "search".into(),
                                    ],
                                };
                                if let Ok(data) = serde_json::to_vec(&msg) {
                                    let topic = gossipsub::IdentTopic::new(TOPIC_HEARTBEAT);
                                    let _ = swarm.behaviour_mut().gossipsub.publish(topic, data);
                                }
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
                            capabilities: vec![
                                "wasm".into(),
                                "llm".into(),
                                "search".into(),
                            ],
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

    /// Shut down the swarm node.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.cmd_tx
            .send(SwarmCommand::Shutdown)
            .await
            .map_err(|e| format!("Swarm channel closed: {}", e))
    }
}
