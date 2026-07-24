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
    /// Port to listen on for inbound QUIC connections.
    pub listen_port: u16,
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
        assert!(config.peer.is_empty());
        assert!(config.mapping.is_empty());
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
        assert_eq!(config.mapping.len(), 2);
        assert_eq!(config.mapping[0].direction, "both");
        assert_eq!(config.mapping[1].signal_filter, vec!["param"]);
    }
}