//! Configuration for the remote bridge.
//!
//! Loaded from a TOML file (default: `bridge-remote.toml`).

use serde::Deserialize;

/// Top-level bridge configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Bridge identity and listener settings.
    pub bridge: BridgeConfig,
    /// Local hub connection settings.
    #[serde(default)]
    pub local: LocalConfig,
    /// Remote peers to connect to.
    #[serde(default)]
    pub peer: Vec<PeerConfig>,
    /// Address mapping rules.
    #[serde(default)]
    pub mapping: Vec<MappingConfig>,
}

/// Bridge identity and listener.
#[derive(Debug, Clone, Deserialize)]
pub struct BridgeConfig {
    /// Display name for this bridge voice.
    pub name: String,
    /// Address to bind the QUIC listener to: an IPv4 or IPv6 literal
    /// (brackets optional for IPv6) or a resolvable hostname.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    /// Port to listen on for inbound QUIC connections.
    pub listen_port: u16,
    /// Optional shared secret that peers must present in the bridge
    /// handshake. When unset, authentication is disabled — only run
    /// without it on trusted networks.
    #[serde(default)]
    pub auth_token: Option<String>,
    /// Maximum number of simultaneously open inbound connections; further
    /// connections are closed at accept time.
    #[serde(default = "default_max_inbound")]
    pub max_inbound: usize,
}

fn default_listen_addr() -> String {
    "0.0.0.0".to_string()
}

fn default_max_inbound() -> usize {
    32
}

/// Local hub connection.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct LocalConfig {
    /// Explicit hub port. If set, bypasses discovery.
    pub port: Option<u16>,
}

/// A remote peer to connect to.
#[derive(Debug, Clone, Deserialize)]
pub struct PeerConfig {
    /// Hostname or IP address.
    pub host: String,
    /// Port number.
    pub port: u16,
    /// Whether to reconnect on disconnect.
    #[serde(default = "default_true")]
    pub reconnect: bool,
    /// Whether to replay current param state to this peer after handshake.
    #[serde(default = "default_true")]
    pub replay_params: bool,
}

fn default_true() -> bool {
    true
}

/// An address mapping rule.
#[derive(Debug, Clone, Deserialize)]
pub struct MappingConfig {
    /// Ensemble routing pattern to match against.
    pub from_pattern: String,
    /// Output address template (may use `{capture}` and `**` passthrough).
    pub to_template: String,
    /// Direction: "outbound", "inbound", or "both".
    pub direction: String,
    /// Optional signal type filter (e.g. ["param", "stream"]).
    #[serde(default)]
    pub signal_filter: Vec<String>,
}

impl Config {
    /// Load configuration from a TOML file.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
[bridge]
name = "test-bridge"
listen_port = 7400
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.bridge.name, "test-bridge");
        assert_eq!(config.bridge.listen_port, 7400);
        // Listener/auth fields default when unset.
        assert_eq!(config.bridge.listen_addr, "0.0.0.0");
        assert_eq!(config.bridge.auth_token, None);
        assert_eq!(config.bridge.max_inbound, 32);
        assert!(config.peer.is_empty());
        assert!(config.mapping.is_empty());
    }

    #[test]
    fn parse_listener_and_auth_fields() {
        let toml = r#"
[bridge]
name = "test-bridge"
listen_addr = "::1"
listen_port = 7400
auth_token = "s3cret"
max_inbound = 4
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.bridge.listen_addr, "::1");
        assert_eq!(config.bridge.auth_token, Some("s3cret".to_string()));
        assert_eq!(config.bridge.max_inbound, 4);
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
[bridge]
name = "site-a"
listen_port = 7400

[local]
port = 7331

[[peer]]
host = "192.168.1.100"
port = 7400
reconnect = true

[[mapping]]
from_pattern = "/transport/**"
to_template = "/remote/transport/**"
direction = "both"

[[mapping]]
from_pattern = "/track/{id}/volume"
to_template = "/mixer/{id}/gain"
direction = "outbound"
signal_filter = ["param"]
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.peer.len(), 1);
        assert_eq!(config.peer[0].host, "192.168.1.100");
        assert!(config.peer[0].reconnect);
        assert!(config.peer[0].replay_params);
        assert_eq!(config.mapping.len(), 2);
        assert_eq!(config.mapping[0].direction, "both");
        assert_eq!(config.mapping[1].signal_filter, vec!["param"]);
    }
}
