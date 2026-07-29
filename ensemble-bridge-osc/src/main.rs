//! Ensemble OSC Bridge — translates between Ensemble actions and OSC/UDP.
//!
//! Connects to the hub as a bridge voice and:
//! - Subscribes recursively to an Ensemble address prefix (default: `/osc/out/**`)
//! - Forwards matching actions as OSC messages via UDP
//! - Listens for inbound OSC messages on a UDP port
//! - Publishes received OSC as Ensemble actions under `/osc/in`
//!
//! # CLI Arguments
//!
//! ```text
//! ensemble-bridge-osc [options]
//!
//! Options:
//!   --name <name>            Voice name shown in the hub (default: osc-bridge)
//!   --ens-prefix <addr>      Ensemble prefix for outbound (default: /osc/out)
//!   --osc-prefix <addr>      OSC prefix for inbound→outbound mapping (default: "")
//!   --osc-send-host <host>   Host to send OSC to (default: 127.0.0.1)
//!   --osc-send-port <port>   Port to send OSC to (default: 9000)
//!   --osc-listen-port <port> UDP port to listen on (default: 9001)
//!   --hub <port>             Explicit hub port (bypasses discovery)
//! ```
//!
//! # Usage
//!
//! ```bash
//! # Default: listen on 9001, send to 9000
//! cargo run --bin ensemble-bridge-osc
//!
//! # Custom configuration for SuperCollider
//! cargo run --bin ensemble-bridge-osc -- --name sc-bridge --osc-send-port 57120 --osc-listen-port 57121
//! ```

mod convert;
mod udp;

use std::net::UdpSocket;

use anyhow::Result;
use ensemble_client::Hub;
use ensemble_core::protocol::*;
use tokio::sync::mpsc;

use convert::{osc_to_ensemble_value, to_osc_message, translate_address_inbound};
use udp::{spawn_udp_listener, spawn_udp_sender, UdpSendCmd};

// ---------------------------------------------------------------------------
// CLI configuration
// ---------------------------------------------------------------------------

/// Configuration parsed from CLI arguments.
struct Config {
    /// Voice name for the hub.
    name: String,
    /// Ensemble address prefix for outbound actions.
    ens_prefix: String,
    /// OSC address prefix for mapping.
    osc_prefix: String,
    /// Host to send OSC messages to.
    osc_send_host: String,
    /// Port to send OSC messages to.
    osc_send_port: u16,
    /// UDP port to listen for inbound OSC.
    osc_listen_port: u16,
    /// Explicit hub port (bypasses discovery).
    hub_port: Option<u16>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: "osc-bridge".to_string(),
            ens_prefix: "/osc/out".to_string(),
            osc_prefix: String::new(),
            osc_send_host: "127.0.0.1".to_string(),
            osc_send_port: 9000,
            osc_listen_port: 9001,
            hub_port: None,
        }
    }
}

impl Config {
    /// Parse CLI arguments into a Config.
    fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut config = Config::default();

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--name" => {
                    if i + 1 < args.len() {
                        config.name = args[i + 1].clone();
                        i += 1;
                    }
                }
                "--ens-prefix" => {
                    if i + 1 < args.len() {
                        config.ens_prefix = args[i + 1].clone();
                        i += 1;
                    }
                }
                "--osc-prefix" => {
                    if i + 1 < args.len() {
                        config.osc_prefix = args[i + 1].clone();
                        i += 1;
                    }
                }
                "--osc-send-host" => {
                    if i + 1 < args.len() {
                        config.osc_send_host = args[i + 1].clone();
                        i += 1;
                    }
                }
                "--osc-send-port" => {
                    if i + 1 < args.len() {
                        if let Ok(port) = args[i + 1].parse() {
                            config.osc_send_port = port;
                        }
                        i += 1;
                    }
                }
                "--osc-listen-port" => {
                    if i + 1 < args.len() {
                        if let Ok(port) = args[i + 1].parse() {
                            config.osc_listen_port = port;
                        }
                        i += 1;
                    }
                }
                "--hub" => {
                    if i + 1 < args.len() {
                        if let Ok(port) = args[i + 1].parse() {
                            config.hub_port = Some(port);
                        }
                        i += 1;
                    }
                }
                _ => {
                    eprintln!("Unknown argument: {}", args[i]);
                }
            }
            i += 1;
        }

        config
    }

    /// Get the target address for sending OSC messages.
    fn osc_target_addr(&self) -> String {
        format!("{}:{}", self.osc_send_host, self.osc_send_port)
    }
}

// ---------------------------------------------------------------------------
// Action router (Ensemble → OSC)
// ---------------------------------------------------------------------------

/// Process incoming Ensemble actions and forward them as OSC messages.
async fn run_action_router(
    mut hub: Hub,
    udp_tx: mpsc::Sender<UdpSendCmd>,
    ens_prefix: String,
    osc_prefix: String,
) {
    while let Some(action_msg) = hub.recv_action().await {
        // The client also forwards the hub's unset_param broadcasts on this
        // channel; only translate genuine actions so an unset doesn't emit a
        // phantom OSC Nil message.
        if action_msg.msg_type != MSG_ACTION {
            continue;
        }
        let map = match &action_msg.payload {
            Value::Map(m) => m.clone(),
            _ => continue,
        };
        let address = get_string(&map, "address").unwrap_or_default();
        let payload = get_value(&map, "payload").unwrap_or(Value::Null);

        // Translate the Ensemble action to an OSC message.
        if let Some(osc_msg) = to_osc_message(&address, &payload, &ens_prefix, &osc_prefix) {
            eprintln!("  → OSC: {} {:?}", osc_msg.addr, osc_msg.args);
            let _ = udp_tx.send(UdpSendCmd::Send(osc_msg)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_args();

    eprintln!("Ensemble OSC Bridge");
    eprintln!("  Voice name: {}", config.name);
    eprintln!("  Ensemble prefix: {}", config.ens_prefix);
    eprintln!(
        "  OSC prefix: {}",
        if config.osc_prefix.is_empty() {
            "(none)"
        } else {
            &config.osc_prefix
        }
    );
    eprintln!(
        "  OSC send: {}:{}",
        config.osc_send_host, config.osc_send_port
    );
    eprintln!("  OSC listen: {}", config.osc_listen_port);

    // Connect to hub.
    let hub = if let Some(port) = config.hub_port {
        eprintln!("Connecting to hub on port {}...", port);
        Hub::connect(port, &config.name).await?
    } else {
        eprintln!("Discovering hub...");
        Hub::connect_with_discovery(&config.name).await?
    };
    eprintln!("Connected to hub as voice #{}", hub.voice_id);

    // Subscribe to the Ensemble prefix for outbound actions. The recursive
    // wildcard covers arbitrarily deep addresses below the prefix.
    let subscribe_pattern = format!("{}/**", config.ens_prefix.trim_end_matches('/'));
    hub.subscribe(&subscribe_pattern).await?;
    eprintln!("Subscribed to: {}", subscribe_pattern);

    // Create UDP socket for sending (bind to any available port).
    let send_socket = UdpSocket::bind("127.0.0.1:0")?;
    let target_addr = config.osc_target_addr();
    eprintln!("UDP sender ready → {}", target_addr);

    // Spawn UDP sender task.
    let udp_tx = spawn_udp_sender(send_socket, target_addr);

    // Spawn UDP listener task.
    let udp_rx = spawn_udp_listener(config.osc_listen_port)?;
    eprintln!("UDP listener ready on port {}", config.osc_listen_port);

    // Get a sender handle for forwarding inbound OSC messages to the hub.
    // This allows us to send actions from the inbound task without holding &Hub.
    let hub_sender = hub.sender();

    // Process inbound OSC messages in a separate task.
    let osc_prefix_clone = config.osc_prefix.clone();
    let ens_in_prefix = "/osc/in".to_string();
    tokio::spawn(async move {
        loop {
            // std::sync::mpsc::Receiver::recv() blocks, so we use try_recv in a loop
            // with a short sleep to avoid busy-waiting.
            match udp_rx.try_recv() {
                Ok(received) => {
                    let ens_addr = translate_address_inbound(
                        &received.message.addr,
                        &osc_prefix_clone,
                        &ens_in_prefix,
                    );
                    let value = osc_to_ensemble_value(&received.message.args);
                    eprintln!("  ← OSC: {} (from {})", ens_addr, received.src_addr);

                    // Forward the OSC message to the hub as an Ensemble action.
                    let action_msg = action(
                        &ens_addr,
                        SignalType::Event,
                        0.0, // immediate
                        value,
                    );
                    if let Err(e) = hub_sender.send(action_msg).await {
                        eprintln!("Failed to send inbound OSC to hub: {}", e);
                        break;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }
    });

    // Run the action router (blocks until hub disconnects).
    run_action_router(hub, udp_tx, config.ens_prefix, config.osc_prefix).await;

    eprintln!("OSC bridge shutting down.");
    Ok(())
}
