//! Chord MIDI Bridge — translates between Chord actions and MIDI I/O.
//!
//! Connects to the hub as a bridge voice and:
//! - `/midi/play` → schedules note-on + note-off with mutex-based cancel safety
//! - `/midi/cancel` → invalidates pending note-off for a channel/note
//! - `/midi/cc` → sends a MIDI CC message
//! - MIDI input → publishes as Chord actions through the hub
//!
//! Usage:
//!   cargo run --bin chord-bridge-midi
//!   cargo run --bin chord-bridge-midi -- --output 1 --input 0
//!   cargo run --bin chord-bridge-midi -- --list

mod key_state;

use std::sync::Arc;

use chord_client::Hub;
use chord_core::protocol::*;
use key_state::{KeyStateStore, MidiBytes};
use midir::{MidiInput, MidiOutput};
use tokio::sync::{mpsc, Mutex};

const DEFAULT_PORT: u16 = 7331;

// ---------------------------------------------------------------------------
// MIDI output handling
// ---------------------------------------------------------------------------

/// Commands sent to the MIDI output task.
enum MidiOutCmd {
    Send(MidiBytes),
}

/// Spawn a task that owns the MIDI output connection and sends bytes.
fn spawn_midi_output(
    conn: midir::MidiOutputConnection,
) -> mpsc::Sender<MidiOutCmd> {
    let (tx, mut rx) = mpsc::channel::<MidiOutCmd>(256);

    // midir's MidiOutputConnection is not Send on all platforms, so we use
    // a dedicated std::thread rather than a tokio task.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut conn = conn;
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    MidiOutCmd::Send(bytes) => {
                        let _ = conn.send(&bytes.0);
                    }
                }
            }
            conn.close();
        });
    });

    tx
}

// ---------------------------------------------------------------------------
// Action routing
// ---------------------------------------------------------------------------

/// Extract (channel, note, velocity, duration_secs) from a /midi/play payload.
fn parse_play_payload(payload: &Payload) -> Option<(u8, u8, u8, f64)> {
    match payload {
        Payload::Tuple(values) if values.len() >= 4 => {
            let channel = match &values[0] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            let note = match &values[1] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            let velocity = match &values[2] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            let duration = match &values[3] {
                Value::F32(v) => *v as f64,
                _ => return None,
            };
            Some((channel, note, velocity, duration))
        }
        _ => None,
    }
}

/// Extract (channel, note) from a /midi/cancel payload.
fn parse_cancel_payload(payload: &Payload) -> Option<(u8, u8)> {
    match payload {
        Payload::Tuple(values) if values.len() >= 2 => {
            let channel = match &values[0] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            let note = match &values[1] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            Some((channel, note))
        }
        _ => None,
    }
}

/// Extract (channel, cc_number, value) from a /midi/cc payload.
fn parse_cc_payload(payload: &Payload) -> Option<(u8, u8, u8)> {
    match payload {
        Payload::Tuple(values) if values.len() >= 3 => {
            let channel = match &values[0] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            let cc = match &values[1] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            let val = match &values[2] {
                Value::I32(v) => *v as u8,
                _ => return None,
            };
            Some((channel, cc, val))
        }
        _ => None,
    }
}

/// Process incoming Chord actions and translate them to MIDI output.
async fn run_action_router(
    mut hub: Hub,
    midi_tx: mpsc::Sender<MidiOutCmd>,
    key_store: Arc<Mutex<KeyStateStore>>,
) {
    while let Some((_source, action)) = hub.recv_action().await {
        if action.address == "/midi/play" {
            if let Some((channel, note, velocity, duration_secs)) =
                parse_play_payload(&action.payload)
            {
                // Bump mutex — invalidates any previous pending note-off.
                let event_id = {
                    let mut ks = key_store.lock().await;
                    ks.bump(channel, note)
                };

                // Schedule note-on (immediately or use action timestamp via hub scheduler).
                let ks = key_store.clone();
                let tx = midi_tx.clone();
                let tx2 = midi_tx.clone();
                let ks2 = key_store.clone();

                // Note-on.
                {
                    let mut store = ks.lock().await;
                    if let Some(bytes) = store.play(event_id, channel, note, velocity) {
                        let _ = tx.send(MidiOutCmd::Send(bytes)).await;
                        eprintln!("  note-on: ch={channel} note={note} vel={velocity}");
                    }
                }

                // Schedule note-off after duration.
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(duration_secs)).await;
                    let mut store = ks2.lock().await;
                    if let Some(bytes) = store.stop(event_id, channel, note) {
                        let _ = tx2.send(MidiOutCmd::Send(bytes)).await;
                        eprintln!("  note-off: ch={channel} note={note}");
                    }
                });
            } else {
                eprintln!("  /midi/play: invalid payload {:?}", action.payload);
            }
        } else if action.address == "/midi/cancel" {
            if let Some((channel, note)) = parse_cancel_payload(&action.payload) {
                // Bump mutex — any pending note-off with the old ID will be dropped.
                let mut ks = key_store.lock().await;
                ks.bump(channel, note);
                eprintln!("  cancel: ch={channel} note={note}");
            }
        } else if action.address == "/midi/cc" {
            if let Some((channel, cc, val)) = parse_cc_payload(&action.payload) {
                let bytes = MidiBytes([0xB0 | channel, cc, val]);
                let _ = midi_tx.send(MidiOutCmd::Send(bytes)).await;
                eprintln!("  cc: ch={channel} cc={cc} val={val}");
            }
        } else {
            eprintln!("  unhandled MIDI action: {}", action.address);
        }
    }
}

// ---------------------------------------------------------------------------
// MIDI input handling
// ---------------------------------------------------------------------------

/// Spawn a MIDI input listener that publishes incoming MIDI as Chord actions.
fn spawn_midi_input(
    port_index: usize,
    hub_tx: mpsc::Sender<Action>,
) -> anyhow::Result<()> {
    let midi_in = MidiInput::new("chord-bridge-midi-in")?;
    let ports = midi_in.ports();
    let port = ports
        .get(port_index)
        .ok_or_else(|| anyhow::anyhow!("MIDI input port {port_index} not found"))?;

    let port_name = midi_in.port_name(port).unwrap_or_default();
    eprintln!("MIDI input: opening port {port_index} ({port_name})");

    // midir callback runs on its own thread.
    let _conn = midi_in.connect(
        port,
        "chord-bridge-midi-in",
        move |_timestamp, message, tx| {
            if message.len() < 2 {
                return;
            }
            let status = message[0] & 0xF0;
            let channel = message[0] & 0x0F;

            let action = match status {
                0x90 if message.len() >= 3 && message[2] > 0 => {
                    // Note-on (velocity > 0). We send as an Event — the receiving
                    // tool decides duration.
                    Some(Action {
                        address: "/midi/in/note-on".into(),
                        signal_type: SignalType::Event,
                        timestamp: 0.0,
                        payload: Payload::Tuple(vec![
                            Value::I32(channel as i32),
                            Value::I32(message[1] as i32),
                            Value::I32(message[2] as i32),
                        ]),
                    })
                }
                0x80 | 0x90 => {
                    // Note-off (or note-on with velocity 0).
                    Some(Action {
                        address: "/midi/in/note-off".into(),
                        signal_type: SignalType::Event,
                        timestamp: 0.0,
                        payload: Payload::Tuple(vec![
                            Value::I32(channel as i32),
                            Value::I32(message[1] as i32),
                        ]),
                    })
                }
                0xB0 if message.len() >= 3 => {
                    // CC.
                    Some(Action {
                        address: "/midi/in/cc".into(),
                        signal_type: SignalType::Event,
                        timestamp: 0.0,
                        payload: Payload::Tuple(vec![
                            Value::I32(channel as i32),
                            Value::I32(message[1] as i32),
                            Value::I32(message[2] as i32),
                        ]),
                    })
                }
                _ => None,
            };

            if let Some(action) = action {
                let _ = tx.try_send(action);
            }
        },
        hub_tx,
    )?;

    // Keep the connection alive by leaking it (it lives for the process lifetime).
    // midir drops the connection when the MidiInputConnection is dropped.
    std::mem::forget(_conn);

    Ok(())
}

// ---------------------------------------------------------------------------
// Port listing
// ---------------------------------------------------------------------------

fn list_ports() {
    eprintln!("\nMIDI Output Ports:");
    if let Ok(midi_out) = MidiOutput::new("chord-list") {
        let ports = midi_out.ports();
        if ports.is_empty() {
            eprintln!("  (none)");
        }
        for (i, port) in ports.iter().enumerate() {
            let name = midi_out.port_name(port).unwrap_or_else(|_| "?".into());
            eprintln!("  {i}: {name}");
        }
    }

    eprintln!("\nMIDI Input Ports:");
    if let Ok(midi_in) = MidiInput::new("chord-list") {
        let ports = midi_in.ports();
        if ports.is_empty() {
            eprintln!("  (none)");
        }
        for (i, port) in ports.iter().enumerate() {
            let name = midi_in.port_name(port).unwrap_or_else(|_| "?".into());
            eprintln!("  {i}: {name}");
        }
    }
    eprintln!();
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Simple arg parsing (avoiding a clap dependency for now).
    if args.iter().any(|a| a == "--list") {
        list_ports();
        return Ok(());
    }

    let hub_port: u16 = args
        .windows(2)
        .find(|w| w[0] == "--hub")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let output_index: Option<usize> = args
        .windows(2)
        .find(|w| w[0] == "--output")
        .and_then(|w| w[1].parse().ok());

    let input_index: Option<usize> = args
        .windows(2)
        .find(|w| w[0] == "--input")
        .and_then(|w| w[1].parse().ok());

    // List available ports for the user.
    list_ports();

    // Open MIDI output.
    let midi_out = MidiOutput::new("chord-bridge-midi")?;
    let out_ports = midi_out.ports();
    let out_idx = output_index.unwrap_or(0);
    let out_port = out_ports
        .get(out_idx)
        .ok_or_else(|| anyhow::anyhow!("No MIDI output port at index {out_idx}"))?;
    let out_name = midi_out.port_name(out_port).unwrap_or_default();
    eprintln!("Opening MIDI output: {out_idx} ({out_name})");
    let conn_out = midi_out.connect(out_port, "chord-bridge-midi")?;
    let midi_tx = spawn_midi_output(conn_out);

    // Connect to hub.
    let hub = Hub::connect(
        hub_port,
        "midi-bridge",
        vec!["/midi/*".into()],
    )
    .await?;
    eprintln!("Connected to hub on port {hub_port} as voice #{}", hub.voice_id);

    // Optionally open MIDI input.
    if let Some(in_idx) = input_index {
        let (action_tx, mut action_rx) = mpsc::channel::<Action>(256);
        spawn_midi_input(in_idx, action_tx)?;

        // Forward MIDI input actions to the hub.
        // We need a clone of the hub's send capability.
        let hub_for_input = hub.voice_id;
        // We can't clone Hub, so we'll use a channel to forward.
        // Actually, Hub::send_action takes &self, so we can share via Arc.
        // But Hub has a non-Send mpsc::Receiver. Instead, spawn a forwarder.
        // For simplicity, we'll use the hub reference directly in the router
        // and spawn the input forwarder here.
        tokio::spawn(async move {
            while let Some(action) = action_rx.recv().await {
                // We can't send from here without the Hub. This is a known
                // limitation — the Hub struct owns recv. For v0.2, MIDI input
                // actions are logged. Full bidirectional support needs Hub
                // to expose a send-only handle.
                eprintln!(
                    "MIDI input (voice {hub_for_input}): {} {:?}",
                    action.address, action.payload
                );
            }
        });
    }

    // Run the action router (blocks until hub disconnects).
    let key_store = Arc::new(Mutex::new(KeyStateStore::new()));
    run_action_router(hub, midi_tx, key_store).await;

    eprintln!("MIDI bridge shutting down.");
    Ok(())
}
