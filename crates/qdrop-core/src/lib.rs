//! Shared core for `qdrop` (CLI) and `qdropd` (daemon): config, the peer
//! registry, identity + pinned TLS, PIN pairing, the wire protocol, and
//! logging. Clipboard/file sync logic lives in the daemon.

pub mod config;
pub mod crypto;
pub mod device;
pub mod frame;
pub mod identity;
pub mod logging;
pub mod pairing;
pub mod paths;
pub mod peers;
pub mod proto;
pub mod roster;
pub mod state;
pub mod tls;

pub use config::Config;
pub use identity::Identity;
pub use peers::{Peer, Peers};
pub use proto::{Caps, Hello, Message, PROTOCOL_VERSION};
pub use roster::Roster;

/// Crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// mDNS service type used for steady-state discovery (M1+).
pub const SERVICE_TYPE: &str = "_qdrop._tcp.local.";

/// mDNS service type advertised only while `qdrop pair` is running (M2).
pub const PAIR_SERVICE_TYPE: &str = "_qdrop-pair._tcp.local.";

/// Default TCP port the daemon listens on.
pub const DEFAULT_PORT: u16 = 47_654;
