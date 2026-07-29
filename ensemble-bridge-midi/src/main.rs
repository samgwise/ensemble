//! Ensemble MIDI Bridge — translates between Ensemble actions and MIDI I/O.
//!
//! Connects to the hub as a bridge voice and:
//! - `/midi/play` → schedules note-on + note-off with mutex-based cancel safety
//! - `/midi/cancel` → invalidates pending note-off for a channel/note
//! - `/midi/cc` → sends a MIDI CC message
//! - MIDI input → publishes as Ensemble actions through the hub
//!
//! All action payloads are range-validated at parse time (channel 0-15;
//! note/cc/value 0-127; finite, non-negative durations), so malformed actions
//! can never panic the bridge or corrupt the MIDI stream.
//!
//! MIDI input forwarding is best-effort: the driver callback must never block,
//! so inbound events are queued with `try_send` and dropped when the buffer is
//! full rather than back-pressuring the MIDI driver.
//!
//! Usage:
//!   cargo run --bin ensemble-bridge-midi
//!   cargo run --bin ensemble-bridge-midi -- --output 1 --input 0
//!   cargo run --bin ensemble-bridge-midi -- --list

mod key_state;

use std::sync::Arc;

use ensemble_client::Hub;
use ensemble_core::protocol::*;
use key_state::{KeyStateStore, MidiBytes};
use midir::{MidiInput, MidiOutput};
use tokio::sync::{mpsc, Mutex};

// ---------------------------------------------------------------------------
// MIDI output handling
// ---------------------------------------------------------------------------

/// Commands sent to the MIDI output task.
enum MidiOutCmd {
    Send(MidiBytes),
}

/// Spawn a task that owns the MIDI output connection and sends bytes.
fn spawn_midi_output(conn: midir::MidiOutputConnection) -> mpsc::Sender<MidiOutCmd> {
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

/// Extract a MIDI channel (0-15) from a value.
///
/// Anything wider would turn the status byte into a non-channel message
/// (e.g. `0x90 | 200`), corrupting the MIDI stream, so out-of-range values
/// are rejected rather than truncated.
fn parse_channel(value: &Value) -> Option<u8> {
    match value {
        Value::Integer(v) if (0..=15).contains(v) => Some(*v as u8),
        _ => None,
    }
}

/// Extract a 7-bit MIDI data byte (0-127) from a value.
fn parse_data_byte(value: &Value) -> Option<u8> {
    match value {
        Value::Integer(v) if (0..=127).contains(v) => Some(*v as u8),
        _ => None,
    }
}

/// Extract a note duration in seconds from a value.
///
/// `Duration::from_secs_f64` panics on negative or non-finite input, so both
/// are rejected here at parse time.
fn parse_duration(value: &Value) -> Option<f64> {
    match value {
        Value::Float(v) => {
            let secs = v.value();
            (secs.is_finite() && secs >= 0.0).then_some(secs)
        }
        _ => None,
    }
}

/// Extract (channel, note, velocity, duration_secs) from a /midi/play payload.
///
/// All fields are range-validated (see the helpers above); `None` is returned
/// for any out-of-range field.
fn parse_play_payload(payload: &Value) -> Option<(u8, u8, u8, f64)> {
    match payload {
        Value::Tuple(values) if values.len() >= 4 => {
            let channel = parse_channel(&values[0])?;
            let note = parse_data_byte(&values[1])?;
            let velocity = parse_data_byte(&values[2])?;
            let duration = parse_duration(&values[3])?;
            Some((channel, note, velocity, duration))
        }
        _ => None,
    }
}

/// Extract (channel, note) from a /midi/cancel payload.
///
/// Fields are range-validated identically to /midi/play.
fn parse_cancel_payload(payload: &Value) -> Option<(u8, u8)> {
    match payload {
        Value::Tuple(values) if values.len() >= 2 => {
            let channel = parse_channel(&values[0])?;
            let note = parse_data_byte(&values[1])?;
            Some((channel, note))
        }
        _ => None,
    }
}

/// Extract (channel, cc_number, value) from a /midi/cc payload.
///
/// Fields are range-validated identically to /midi/play.
fn parse_cc_payload(payload: &Value) -> Option<(u8, u8, u8)> {
    match payload {
        Value::Tuple(values) if values.len() >= 3 => {
            let channel = parse_channel(&values[0])?;
            let cc = parse_data_byte(&values[1])?;
            let val = parse_data_byte(&values[2])?;
            Some((channel, cc, val))
        }
        _ => None,
    }
}

/// Process incoming Ensemble actions and translate them to MIDI output.
async fn run_action_router(
    mut hub: Hub,
    midi_tx: mpsc::Sender<MidiOutCmd>,
    key_store: Arc<Mutex<KeyStateStore>>,
) {
    while let Some(action_msg) = hub.recv_action().await {
        let map = match &action_msg.payload {
            Value::Map(m) => m.clone(),
            _ => continue,
        };
        let address = get_string(&map, "address").unwrap_or_default();
        let payload = get_value(&map, "payload").unwrap_or(Value::Null);

        if address == "/midi/play" {
            if let Some((channel, note, velocity, duration_secs)) = parse_play_payload(&payload) {
                // Bump mutex — invalidates any previous pending note-off.
                let event_id = {
                    let mut ks = key_store.lock().await;
                    ks.bump(channel, note)
                };

                // Note-on (immediately; the hub scheduler handles the timing of
                // timestamped actions before they reach us).
                let note_on_fired = {
                    let mut store = key_store.lock().await;
                    if let Some(bytes) = store.play(event_id, channel, note, velocity) {
                        let _ = midi_tx.send(MidiOutCmd::Send(bytes)).await;
                        eprintln!("  note-on: ch={channel} note={note} vel={velocity}");
                        true
                    } else {
                        false
                    }
                };

                // Schedule note-off after the duration — only when the note-on
                // actually fired, so a superseded play can never cut short a
                // note it did not start.
                if note_on_fired {
                    let ks2 = key_store.clone();
                    let tx2 = midi_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs_f64(duration_secs)).await;
                        let mut store = ks2.lock().await;
                        if let Some(bytes) = store.stop(event_id, channel, note) {
                            let _ = tx2.send(MidiOutCmd::Send(bytes)).await;
                            eprintln!("  note-off: ch={channel} note={note}");
                        }
                    });
                }
            } else {
                eprintln!("  /midi/play: invalid payload {payload:?}");
            }
        } else if address == "/midi/cancel" {
            if let Some((channel, note)) = parse_cancel_payload(&payload) {
                // Bump mutex — any pending note-off with the old ID will be dropped.
                let mut ks = key_store.lock().await;
                ks.bump(channel, note);
                eprintln!("  cancel: ch={channel} note={note}");
            } else {
                eprintln!("  /midi/cancel: invalid payload {payload:?}");
            }
        } else if address == "/midi/cc" {
            if let Some((channel, cc, val)) = parse_cc_payload(&payload) {
                let bytes = MidiBytes([0xB0 | channel, cc, val]);
                let _ = midi_tx.send(MidiOutCmd::Send(bytes)).await;
                eprintln!("  cc: ch={channel} cc={cc} val={val}");
            } else {
                eprintln!("  /midi/cc: invalid payload {payload:?}");
            }
        } else {
            eprintln!("  unhandled MIDI action: {}", address);
        }
    }
}

// ---------------------------------------------------------------------------
// MIDI input handling
// ---------------------------------------------------------------------------

/// Spawn a MIDI input listener that publishes incoming MIDI as Ensemble actions.
///
/// Forwarding is best-effort: the midir callback runs on a real-time driver
/// thread that must never block, so messages are queued with `try_send` and
/// silently dropped when the channel is full. Sustained input bursts can
/// therefore lose events rather than back-pressuring the driver.
fn spawn_midi_input(port_index: usize, hub_tx: mpsc::Sender<WireMessage>) -> anyhow::Result<()> {
    let midi_in = MidiInput::new("ensemble-bridge-midi-in")?;
    let ports = midi_in.ports();
    let port = ports
        .get(port_index)
        .ok_or_else(|| anyhow::anyhow!("MIDI input port {port_index} not found"))?;

    let port_name = midi_in.port_name(port).unwrap_or_default();
    eprintln!("MIDI input: opening port {port_index} ({port_name})");

    // midir callback runs on its own thread.
    let _conn = midi_in
        .connect(
            port,
            "ensemble-bridge-midi-in",
            move |_timestamp, message, tx| {
                if message.len() < 2 {
                    return;
                }
                let status = message[0] & 0xF0;
                let channel = message[0] & 0x0F;

                let msg = match status {
                    0x90 if message.len() >= 3 && message[2] > 0 => {
                        // Note-on (velocity > 0). We send as an Event — the receiving
                        // tool decides duration.
                        Some(action(
                            "/midi/in/note-on",
                            SignalType::Event,
                            0.0,
                            Value::Tuple(vec![
                                Value::Integer(channel as i64),
                                Value::Integer(message[1] as i64),
                                Value::Integer(message[2] as i64),
                            ]),
                        ))
                    }
                    0x80 | 0x90 => {
                        // Note-off (or note-on with velocity 0).
                        Some(action(
                            "/midi/in/note-off",
                            SignalType::Event,
                            0.0,
                            Value::Tuple(vec![
                                Value::Integer(channel as i64),
                                Value::Integer(message[1] as i64),
                            ]),
                        ))
                    }
                    0xB0 if message.len() >= 3 => {
                        // CC.
                        Some(action(
                            "/midi/in/cc",
                            SignalType::Event,
                            0.0,
                            Value::Tuple(vec![
                                Value::Integer(channel as i64),
                                Value::Integer(message[1] as i64),
                                Value::Integer(message[2] as i64),
                            ]),
                        ))
                    }
                    _ => None,
                };

                if let Some(msg) = msg {
                    // Best-effort: drop on a full buffer rather than block the
                    // driver callback (see the function docs).
                    let _ = tx.try_send(msg);
                }
            },
            hub_tx,
        )
        .map_err(|e| anyhow::anyhow!("MIDI input connection failed: {}", e))?;

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
    if let Ok(midi_out) = MidiOutput::new("ensemble-list") {
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
    if let Ok(midi_in) = MidiInput::new("ensemble-list") {
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

    // When --hub <port> is given, connect explicitly to that port.
    // Otherwise, use automatic port discovery via the hub's port file.
    let explicit_port: Option<u16> = args
        .windows(2)
        .find(|w| w[0] == "--hub")
        .and_then(|w| w[1].parse().ok());

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
    let midi_out = MidiOutput::new("ensemble-bridge-midi")?;
    let out_ports = midi_out.ports();
    let out_idx = output_index.unwrap_or(0);
    let out_port = out_ports
        .get(out_idx)
        .ok_or_else(|| anyhow::anyhow!("No MIDI output port at index {out_idx}"))?;
    let out_name = midi_out.port_name(out_port).unwrap_or_default();
    eprintln!("Opening MIDI output: {out_idx} ({out_name})");
    let conn_out = midi_out
        .connect(out_port, "ensemble-bridge-midi")
        .map_err(|e| anyhow::anyhow!("MIDI output connection failed: {}", e))?;
    let midi_tx = spawn_midi_output(conn_out);

    // Connect to hub — use discovery when no explicit port is given.
    let (hub, hub_port) = if let Some(port) = explicit_port {
        let h = Hub::connect(port, "midi-bridge").await?;
        (h, port)
    } else {
        let h = Hub::connect_with_discovery("midi-bridge").await?;
        eprintln!("Hub discovered via port file");
        (h, 0u16) // port unknown when using discovery
    };
    hub.subscribe("/midi/*").await?;
    if explicit_port.is_some() {
        eprintln!(
            "Connected to hub on port {hub_port} as voice #{}",
            hub.voice_id
        );
    } else {
        eprintln!("Connected to hub as voice #{}", hub.voice_id);
    }

    // Optionally open MIDI input.
    if let Some(in_idx) = input_index {
        let (msg_tx, mut msg_rx) = mpsc::channel::<WireMessage>(256);
        spawn_midi_input(in_idx, msg_tx)?;

        // Get a sender handle for forwarding MIDI input to the hub.
        let hub_sender = hub.sender();
        let hub_for_input = hub.voice_id;

        // Forward MIDI input actions to the hub.
        tokio::spawn(async move {
            while let Some(msg) = msg_rx.recv().await {
                let map = match &msg.payload {
                    Value::Map(m) => m.clone(),
                    _ => continue,
                };
                let address = get_string(&map, "address").unwrap_or_default();
                eprintln!(
                    "MIDI input (voice {hub_for_input}): {} {:?}",
                    address, msg.payload
                );

                // Forward the MIDI input action to the hub.
                if let Err(e) = hub_sender.send(msg).await {
                    eprintln!("Failed to send MIDI input to hub: {}", e);
                    break;
                }
            }
        });
    }

    // Run the action router (blocks until hub disconnects).
    let key_store = Arc::new(Mutex::new(KeyStateStore::new()));
    run_action_router(hub, midi_tx, key_store).await;

    eprintln!("MIDI bridge shutting down.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn play_payload(channel: i64, note: i64, velocity: i64, duration: f64) -> Value {
        Value::Tuple(vec![
            Value::Integer(channel),
            Value::Integer(note),
            Value::Integer(velocity),
            Value::Float(FloatValue::new(duration)),
        ])
    }

    fn cancel_payload(channel: i64, note: i64) -> Value {
        Value::Tuple(vec![Value::Integer(channel), Value::Integer(note)])
    }

    fn cc_payload(channel: i64, cc: i64, val: i64) -> Value {
        Value::Tuple(vec![
            Value::Integer(channel),
            Value::Integer(cc),
            Value::Integer(val),
        ])
    }

    #[test]
    fn play_accepts_valid_payload() {
        let (channel, note, velocity, duration) =
            parse_play_payload(&play_payload(0, 60, 100, 0.5)).unwrap();
        assert_eq!((channel, note, velocity), (0, 60, 100));
        assert!((duration - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn play_accepts_boundary_values() {
        assert!(parse_play_payload(&play_payload(15, 127, 127, 0.0)).is_some());
        assert!(parse_play_payload(&play_payload(0, 0, 0, 0.0)).is_some());
    }

    #[test]
    fn play_rejects_out_of_range_channel() {
        assert!(parse_play_payload(&play_payload(16, 60, 100, 0.5)).is_none());
        assert!(parse_play_payload(&play_payload(-1, 60, 100, 0.5)).is_none());
        assert!(parse_play_payload(&play_payload(i64::MAX, 60, 100, 0.5)).is_none());
    }

    #[test]
    fn play_rejects_out_of_range_note_and_velocity() {
        assert!(parse_play_payload(&play_payload(0, 128, 100, 0.5)).is_none());
        assert!(parse_play_payload(&play_payload(0, -1, 100, 0.5)).is_none());
        assert!(parse_play_payload(&play_payload(0, 60, 128, 0.5)).is_none());
        assert!(parse_play_payload(&play_payload(0, 60, -1, 0.5)).is_none());
    }

    #[test]
    fn play_rejects_bad_duration() {
        // Negative durations would panic `Duration::from_secs_f64`.
        assert!(parse_play_payload(&play_payload(0, 60, 100, -0.5)).is_none());
        // Non-finite durations likewise.
        assert!(parse_play_payload(&play_payload(0, 60, 100, f64::NAN)).is_none());
        assert!(parse_play_payload(&play_payload(0, 60, 100, f64::INFINITY)).is_none());
        assert!(parse_play_payload(&play_payload(0, 60, 100, f64::NEG_INFINITY)).is_none());
    }

    #[test]
    fn play_rejects_wrong_types_and_arity() {
        // Too few elements.
        assert!(parse_play_payload(&Value::Tuple(vec![
            Value::Integer(0),
            Value::Integer(60),
            Value::Integer(100),
        ]))
        .is_none());
        // Duration must be a Float.
        assert!(parse_play_payload(&Value::Tuple(vec![
            Value::Integer(0),
            Value::Integer(60),
            Value::Integer(100),
            Value::Integer(1),
        ]))
        .is_none());
        // Channel must be an Integer.
        assert!(parse_play_payload(&Value::Tuple(vec![
            Value::Float(FloatValue::new(0.0)),
            Value::Integer(60),
            Value::Integer(100),
            Value::Float(FloatValue::new(0.5)),
        ]))
        .is_none());
        // Not a tuple at all.
        assert!(parse_play_payload(&Value::Null).is_none());
    }

    #[test]
    fn cancel_accepts_valid_payload() {
        assert_eq!(parse_cancel_payload(&cancel_payload(0, 60)), Some((0, 60)));
        assert_eq!(
            parse_cancel_payload(&cancel_payload(15, 127)),
            Some((15, 127))
        );
    }

    #[test]
    fn cancel_rejects_out_of_range() {
        assert!(parse_cancel_payload(&cancel_payload(16, 60)).is_none());
        assert!(parse_cancel_payload(&cancel_payload(-1, 60)).is_none());
        assert!(parse_cancel_payload(&cancel_payload(0, 128)).is_none());
        assert!(parse_cancel_payload(&cancel_payload(0, -1)).is_none());
    }

    #[test]
    fn cancel_rejects_wrong_arity() {
        assert!(parse_cancel_payload(&Value::Tuple(vec![Value::Integer(0)])).is_none());
        assert!(parse_cancel_payload(&Value::Null).is_none());
    }

    #[test]
    fn cc_accepts_valid_payload() {
        assert_eq!(parse_cc_payload(&cc_payload(0, 7, 100)), Some((0, 7, 100)));
        assert_eq!(
            parse_cc_payload(&cc_payload(15, 127, 127)),
            Some((15, 127, 127))
        );
    }

    #[test]
    fn cc_rejects_out_of_range() {
        assert!(parse_cc_payload(&cc_payload(16, 7, 100)).is_none());
        assert!(parse_cc_payload(&cc_payload(-1, 7, 100)).is_none());
        assert!(parse_cc_payload(&cc_payload(0, 128, 100)).is_none());
        assert!(parse_cc_payload(&cc_payload(0, -1, 100)).is_none());
        assert!(parse_cc_payload(&cc_payload(0, 7, 128)).is_none());
        assert!(parse_cc_payload(&cc_payload(0, 7, -1)).is_none());
    }

    #[test]
    fn cc_rejects_wrong_types_and_arity() {
        assert!(
            parse_cc_payload(&Value::Tuple(vec![Value::Integer(0), Value::Integer(7),])).is_none()
        );
        assert!(parse_cc_payload(&Value::Tuple(vec![
            Value::Integer(0),
            Value::Integer(7),
            Value::String("100".into()),
        ]))
        .is_none());
        assert!(parse_cc_payload(&Value::Null).is_none());
    }
}
