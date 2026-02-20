//! Peer-to-peer swarm mesh for distributed task execution.
//!
//! Uses libp2p to form a Hive cluster of IronClaw instances.
//! Peers discover each other via mDNS (local) and Kademlia (WAN),
//! communicate via GossipSub, and distribute tasks across the mesh.

pub mod node;
pub mod protocol;
