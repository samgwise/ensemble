//! Ensemble Hub headless binary.
//!
//! Runs the hub server without any TUI. For the TUI, use `ensemble-hub-tui`.

use ensemble_discovery::{delete_port_file, is_port_bound, read_port_file, write_port_file};
use ensemble_hub::start_server;

const DEFAULT_PORT: u16 = 7331;

/// Parse the `--port <port>` CLI argument from the process arguments.
///
/// Returns `Some(port)` when the flag is present with a valid value.
fn parse_port_arg() -> Option<u16> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--port" {
            return args.next().and_then(|v| v.parse().ok());
        }
    }
    None
}

/// Resolve the port to bind to.
///
/// Priority: `--port` CLI argument > `ENSEMBLE_HUB_PORT` env var > default (7331).
fn resolve_port() -> u16 {
    parse_port_arg()
        .or_else(|| {
            std::env::var("ENSEMBLE_HUB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(DEFAULT_PORT)
}

/// Remove a stale port file left over from a previous run whose port is no
/// longer bound.
fn cleanup_stale_port_file() {
    if let Some(port) = read_port_file() {
        if !is_port_bound(port) {
            let _ = delete_port_file();
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Clean up any stale port file from a previous run.
    cleanup_stale_port_file();

    let port = resolve_port();
    let (_state, actual_port) = start_server(port).await?;

    // Publish the actual bound port so clients can discover us.
    write_port_file(actual_port)?;

    eprintln!("Ensemble Hub running headless on 127.0.0.1:{actual_port}");

    // Wait for Ctrl+C, then clean up the port file before exiting.
    tokio::signal::ctrl_c().await?;
    let _ = delete_port_file();

    Ok(())
}
