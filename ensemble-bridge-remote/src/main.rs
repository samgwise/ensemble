//! Ensemble Remote Bridge — connects two Ensemble hubs over QUIC.
//!
//! This binary is a thin wrapper around the `ensemble_bridge_remote` library.

use anyhow::Result;
use ensemble_bridge_remote::{run_bridge, Config};

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration.
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bridge-remote.toml".to_string());

    eprintln!("Loading config from: {}", config_path);
    let config = Config::load(&config_path)?;

    // Run until Ctrl+C.
    run_bridge(config, None, None).await
}
