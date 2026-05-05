//! Chord Hub — the central router and reference clock for the Chord protocol.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use chord_core::protocol::*;
use chord_core::{codec, CodecError};
use tokio::io::{BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

// ---------------------------------------------------------------------------
// Hub state
// ---------------------------------------------------------------------------

/// A connected voice with its metadata and message sender.
struct ConnectedVoice {
    id: VoiceId,
    capabilities: VoiceCapabilities,
    /// Channel to send messages to this voice's writer task.
    tx: mpsc::Sender<Message>,
}

/// A scheduled action waiting to be dispatched at a future hub time.
struct ScheduledAction {
    source: VoiceId,
    action: Action,
}

/// Shared hub state, protected by a mutex.
struct HubState {
    /// Monotonic clock baseline — hub time is seconds since this instant.
    clock_origin: Instant,
    /// Next voice ID to assign.
    next_voice_id: VoiceId,
    /// All currently connected voices.
    voices: HashMap<VoiceId, ConnectedVoice>,
    /// Event log for the TUI (most recent events, capped).
    event_log: Vec<String>,
    /// Scheduled actions ordered by timestamp. Uses a BTreeMap so we can
    /// efficiently pop all actions whose time has arrived. The key is an
    /// ordered-float-like u64 (f64 bits) to keep BTreeMap happy.
    schedule: BTreeMap<u64, Vec<ScheduledAction>>,
    /// Last known value for each Param-type address (for late-joiner replay).
    param_state: HashMap<String, (VoiceId, Action)>,
}

/// Convert f64 timestamp to a sortable u64 key for the BTreeMap.
/// Works correctly for non-negative f64 values (which hub timestamps always are).
fn timestamp_key(t: f64) -> u64 {
    t.to_bits()
}

impl HubState {
    fn new() -> Self {
        Self {
            clock_origin: Instant::now(),
            next_voice_id: 1,
            voices: HashMap::new(),
            event_log: Vec::new(),
            schedule: BTreeMap::new(),
            param_state: HashMap::new(),
        }
    }

    /// Current hub time in seconds (monotonic, starts at 0.0).
    fn now(&self) -> f64 {
        self.clock_origin.elapsed().as_secs_f64()
    }

    /// Add an event to the log (keeps last 100 entries).
    fn log(&mut self, msg: String) {
        if self.event_log.len() >= 100 {
            self.event_log.remove(0);
        }
        self.event_log.push(msg);
    }
}

type SharedState = Arc<Mutex<HubState>>;

// ---------------------------------------------------------------------------
// Voice connection handler
// ---------------------------------------------------------------------------

/// Handle a single voice's TCP connection.
async fn handle_voice(stream: TcpStream, state: SharedState) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".into());

    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut writer = BufWriter::new(writer);

    // Wait for Hello message.
    let hello = match codec::read_message(&mut reader).await {
        Ok(Message::Hello(caps)) => caps,
        Ok(other) => {
            let mut st = state.lock().await;
            st.log(format!("[{peer}] Expected Hello, got {other:?}"));
            return;
        }
        Err(e) => {
            let mut st = state.lock().await;
            st.log(format!("[{peer}] Error reading Hello: {e}"));
            return;
        }
    };

    // Register voice.
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    let voice_id;
    {
        let mut st = state.lock().await;
        voice_id = st.next_voice_id;
        st.next_voice_id += 1;
        let hub_time = st.now();

        st.voices.insert(
            voice_id,
            ConnectedVoice {
                id: voice_id,
                capabilities: hello.clone(),
                tx: tx.clone(),
            },
        );

        st.log(format!(
            "Voice {voice_id} connected: \"{}\" from {peer}",
            hello.name
        ));

        // Send Welcome.
        let welcome = Message::Welcome { voice_id, hub_time };
        if let Err(e) = codec::write_message(&mut writer, &welcome).await {
            st.log(format!("Voice {voice_id}: failed to send Welcome: {e}"));
            st.voices.remove(&voice_id);
            return;
        }

        // Replay current param state to the new voice.
        for (source, action) in st.param_state.values() {
            if matches_any(&hello.subscriptions, &action.address) {
                let msg = Message::ActionMessage {
                    source: *source,
                    action: action.clone(),
                };
                let _ = tx.send(msg).await;
            }
        }
    }

    // Spawn a writer task that forwards messages from the channel to the TCP stream.
    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if codec::write_message(&mut writer, &msg).await.is_err() {
                break;
            }
        }
    });

    // Read loop — process incoming messages from this voice.
    loop {
        match codec::read_message(&mut reader).await {
            Ok(Message::ClockSyncRequest { voice_send_time }) => {
                let st = state.lock().await;
                let hub_receive_time = st.now();
                let hub_send_time = st.now();
                let reply = Message::ClockSyncReply {
                    voice_send_time,
                    hub_receive_time,
                    hub_send_time,
                };
                let _ = tx.send(reply).await;
            }

            Ok(Message::ActionMessage { action, .. }) => {
                let mut st = state.lock().await;

                // Store param state for late-joiner replay.
                if action.signal_type == SignalType::Param {
                    st.param_state.insert(
                        action.address.clone(),
                        (voice_id, action.clone()),
                    );
                }

                // If the action has a future timestamp, schedule it.
                if action.timestamp > 0.0 && action.timestamp > st.now() {
                    let key = timestamp_key(action.timestamp);
                    st.schedule
                        .entry(key)
                        .or_default()
                        .push(ScheduledAction {
                            source: voice_id,
                            action,
                        });
                } else {
                    // Route immediately.
                    route_action(&st, voice_id, &action).await;
                }
            }

            Ok(Message::Subscribe { patterns }) => {
                let mut st = state.lock().await;
                if let Some(voice) = st.voices.get_mut(&voice_id) {
                    voice.capabilities.subscriptions.extend(patterns);
                }
            }

            Ok(Message::Unsubscribe { patterns }) => {
                let mut st = state.lock().await;
                if let Some(voice) = st.voices.get_mut(&voice_id) {
                    voice
                        .capabilities
                        .subscriptions
                        .retain(|s| !patterns.contains(s));
                }
            }

            Ok(Message::Goodbye) => {
                let mut st = state.lock().await;
                st.log(format!("Voice {voice_id} disconnected (Goodbye)"));
                st.voices.remove(&voice_id);
                break;
            }

            Ok(other) => {
                let mut st = state.lock().await;
                st.log(format!("Voice {voice_id}: unexpected message {other:?}"));
            }

            Err(CodecError::ConnectionClosed) => {
                let mut st = state.lock().await;
                st.log(format!("Voice {voice_id} disconnected (connection closed)"));
                st.voices.remove(&voice_id);
                break;
            }

            Err(e) => {
                let mut st = state.lock().await;
                st.log(format!("Voice {voice_id}: read error: {e}"));
                st.voices.remove(&voice_id);
                break;
            }
        }
    }

    writer_handle.abort();
}

// Pattern matching is provided by chord_core::pattern.
use chord_core::pattern::matches_any;

/// Route an action to all subscribed voices (except the sender).
async fn route_action(st: &HubState, source: VoiceId, action: &Action) {
    let msg = Message::ActionMessage {
        source,
        action: action.clone(),
    };
    for voice in st.voices.values() {
        if voice.id == source {
            continue;
        }
        if matches_any(&voice.capabilities.subscriptions, &action.address) {
            let _ = voice.tx.send(msg.clone()).await;
        }
    }
}

/// Background task that polls the schedule queue and dispatches due actions.
async fn run_scheduler(state: SharedState) {
    loop {
        {
            let mut st = state.lock().await;
            let now = st.now();
            let now_key = timestamp_key(now);

            // Collect all keys that are due (timestamp <= now).
            let due_keys: Vec<u64> = st
                .schedule
                .range(..=now_key)
                .map(|(k, _)| *k)
                .collect();

            // Dispatch them.
            for key in due_keys {
                if let Some(actions) = st.schedule.remove(&key) {
                    for scheduled in actions {
                        route_action(&st, scheduled.source, &scheduled.action).await;
                    }
                }
            }
        }
        // Poll every 1ms for tight scheduling.
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

// ---------------------------------------------------------------------------
// TUI
// ---------------------------------------------------------------------------

use crosskeyterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

/// Run the TUI in the current terminal, polling hub state for display.
async fn run_tui(state: SharedState) -> std::io::Result<()> {
    // Set up terminal.
    crosskeyterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crosskeyterm::execute!(
        stdout,
        crosskeyterm::terminal::EnterAlternateScreen
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        // Draw.
        let st = state.lock().await;
        let hub_time = st.now();
        let voice_count = st.voices.len();

        let voice_strings: Vec<String> = st
            .voices
            .values()
            .map(|v| {
                format!(
                    "  #{}: \"{}\" [{}]",
                    v.id,
                    v.capabilities.name,
                    v.capabilities.subscriptions.join(", ")
                )
            })
            .collect();

        let log_strings: Vec<String> = st
            .event_log
            .iter()
            .rev()
            .take(20)
            .cloned()
            .collect();

        drop(st); // Release lock before drawing.

        let voice_items: Vec<ListItem> = voice_strings.iter().map(|s| ListItem::new(s.as_str())).collect();
        let log_items: Vec<ListItem> = log_strings.iter().map(|s| ListItem::new(s.as_str())).collect();

        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Min(5),
                ])
                .split(frame.area());

            // Header.
            let header = Paragraph::new(format!(
                " Chord Hub | time: {hub_time:.2}s | voices: {voice_count} | press 'q' to quit"
            ))
            .block(Block::default().borders(Borders::ALL).title(" Chord "));
            frame.render_widget(header, chunks[0]);

            // Voices list.
            let voices = List::new(voice_items)
                .block(Block::default().borders(Borders::ALL).title(" Voices "));
            frame.render_widget(voices, chunks[1]);

            // Event log.
            let log = List::new(log_items)
                .block(Block::default().borders(Borders::ALL).title(" Events "));
            frame.render_widget(log, chunks[2]);
        })?;

        // Poll for input (non-blocking, 100ms timeout).
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }

    // Restore terminal.
    crosskeyterm::terminal::disable_raw_mode()?;
    crosskeyterm::execute!(
        std::io::stdout(),
        crosskeyterm::terminal::LeaveAlternateScreen
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const DEFAULT_PORT: u16 = 7331;

/// Start the hub's TCP accept loop. Returns the actual port bound to.
/// This is the entrypoint used by both the binary and integration tests.
pub async fn start_server(port: u16) -> anyhow::Result<(SharedState, u16)> {
    let state = Arc::new(Mutex::new(HubState::new()));

    // Bind to port 0 to let the OS pick a free port if requested.
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    let actual_port = listener.local_addr()?.port();

    {
        let mut st = state.lock().await;
        st.log(format!("Hub listening on 127.0.0.1:{actual_port}"));
    }

    let accept_state = state.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let voice_state = accept_state.clone();
                    tokio::spawn(handle_voice(stream, voice_state));
                }
                Err(e) => {
                    let mut st = accept_state.lock().await;
                    st.log(format!("Accept error: {e}"));
                }
            }
        }
    });

    // Spawn the scheduler dispatch task.
    let sched_state = state.clone();
    tokio::spawn(run_scheduler(sched_state));

    Ok((state, actual_port))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port = std::env::var("CHORD_HUB_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let headless = std::env::args().any(|a| a == "--headless");

    let (state, actual_port) = start_server(port).await?;

    if headless {
        eprintln!("Chord Hub running headless on 127.0.0.1:{actual_port}");
        // In headless mode, just wait forever.
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    } else {
        run_tui(state).await?;
    }

    Ok(())
}
