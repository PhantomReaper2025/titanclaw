//! Handler modules for the web gateway API.
//!
//! Chat handlers are the only extracted web handlers currently wired into the
//! live gateway router. Other HTTP surfaces still live in `server.rs`.

pub mod chat;

// Re-export all handler functions so `server.rs` can reference them
// as `handlers::chat_send_handler`, etc.
pub use chat::*;
