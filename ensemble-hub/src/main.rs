//! Ensemble Hub headless binary.
//!
//! Runs the hub server without any TUI. For the TUI, use `ensemble-hub-tui`.

use ensemble_hub::start_server;

const DEFAULT_PORT: u16 = 7331;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port = std::env::var("ENSEMBLE_HUB_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let (_state, actual_port) = start_server(port).await?;

    eprintln!("Ensemble Hub running headless on 127.0.0.1:{actual_port}");

    // Keep the main task alive so the hub stays running.
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}
