//! Shared core for `qdrop` (CLI) and `qdropd` (daemon).
//!
//! For milestone M0 this crate only carries the pieces both binaries need:
//! configuration loading, the peer registry, path resolution, and logging
//! setup. Protocol, transport, and sync logic arrive in later milestones.

pub mod config;
pub mod device;
pub mod frame;
pub mod logging;
pub mod paths;
pub mod peers;
pub mod proto;

pub use config::Config;
pub use peers::{Peer, Peers};
pub use proto::{Caps, Hello, Message, PROTOCOL_VERSION};

/// Crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// mDNS service type used for steady-state discovery (M1+).
pub const SERVICE_TYPE: &str = "_qdrop._tcp.local.";

/// Default TCP port the daemon listens on.
pub const DEFAULT_PORT: u16 = 47_654;
