//! Ensemble Hub library — the central router and reference clock for the Ensemble protocol.
//!
//! This crate provides the hub server implementation without any UI dependencies.
//! It can be used as a library for embedding or run headless as a binary.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use ensemble_core::protocol::*;
use ensemble_core::{codec, CodecError};
use ensemble_routing::{matches_any, Pattern};
use tokio::io::{BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

// ---------------------------------------------------------------------------
// Hub state
// ---------------------------------------------------------------------------

/// A connected voice with its metadata and message sender.
pub struct ConnectedVoice {
    pub id: VoiceId,
    /// Display name from the hello message.
    pub name: String,
    /// Parsed subscription patterns for routing.
    pub subscription_patterns: Vec<Pattern>,
    /// Raw pattern strings, kept in sync with parsed patterns for unsubscribe.
    pub subscription_strings: Vec<String>,
    /// Channel to send messages to this voice's writer task.
    pub tx: mpsc::Sender<WireMessage>,
    /// When this voice connected (hub time).
    pub connected_at: f64,
}

/// A scheduled action waiting to be dispatched at a future hub time.
pub struct ScheduledAction {
    pub source: VoiceId,
    /// The full WireMessage action to dispatch.
    pub message: WireMessage,
    /// Parsed action fields for routing decisions.
    pub address: String,
    /// Signal type for routing decisions (param state activation).
    pub signal_type: SignalType,
    /// The timestamp when this action should be dispatched.
    pub timestamp: f64,
}

/// A log entry for a routed action (for the Action Monitor view).
#[derive(Clone)]
pub struct ActionLogEntry {
    /// Hub time when the action was routed.
    pub timestamp: f64,
    /// Source voice ID.
    pub source: VoiceId,
    /// Source voice name (if known).
    pub source_name: String,
    /// Action address.
    pub address: String,
    /// Signal type.
    pub signal_type: SignalType,
}

/// Shared hub state, protected by a mutex.
pub struct HubState {
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
    param_state: HashMap<String, (VoiceId, WireMessage)>,
    /// Current manifest for each voice (advisory metadata, does not affect routing).
    manifests: HashMap<VoiceId, VoiceManifest>,
    /// Action log ring buffer for the Action Monitor view (most recent actions).
    action_log: VecDeque<ActionLogEntry>,
    /// Maximum size of the action log ring buffer.
    action_log_capacity: usize,
}

/// Convert f64 timestamp to a sortable u64 key for the BTreeMap.
/// Works correctly for non-negative f64 values (which hub timestamps always are).
fn timestamp_key(t: f64) -> u64 {
    t.to_bits()
}

impl HubState {
    /// Create a new hub state with default settings.
    pub fn new() -> Self {
        Self {
            clock_origin: Instant::now(),
            next_voice_id: 1,
            voices: HashMap::new(),
            event_log: Vec::new(),
            schedule: BTreeMap::new(),
            param_state: HashMap::new(),
            manifests: HashMap::new(),
            action_log: VecDeque::new(),
            action_log_capacity: 1000,
        }
    }

    /// Current hub time in seconds (monotonic, starts at 0.0).
    pub fn now(&self) -> f64 {
        self.clock_origin.elapsed().as_secs_f64()
    }

    /// Add an event to the log (keeps last 100 entries).
    fn log(&mut self, msg: String) {
        if self.event_log.len() >= 100 {
            self.event_log.remove(0);
        }
        self.event_log.push(msg);
    }

    /// Add an action to the action log ring buffer.
    fn log_action(&mut self, entry: ActionLogEntry) {
        if self.action_log.len() >= self.action_log_capacity {
            self.action_log.pop_front();
        }
        self.action_log.push_back(entry);
    }

    /// Remove a voice and all its associated state (subscriptions, params, manifest).
    /// Returns the voice name if the voice existed, for hub event emission.
    fn remove_voice(&mut self, voice_id: VoiceId, reason: &str) -> Option<String> {
        self.log(format!("Voice {voice_id} disconnected ({reason})"));
        let voice_name = self.voices.remove(&voice_id).map(|v| v.name);
        // Remove param state owned by this voice.
        self.param_state
            .retain(|_, (source, _)| *source != voice_id);
        // Remove scheduled actions from this voice.
        for actions in self.schedule.values_mut() {
            actions.retain(|sa| sa.source != voice_id);
        }
        // Clean up empty schedule entries.
        self.schedule.retain(|_, actions| !actions.is_empty());
        // Remove manifest for this voice.
        self.manifests.remove(&voice_id);
        voice_name
    }

    // -----------------------------------------------------------------------
    // Public accessors for TUI
    // -----------------------------------------------------------------------

    /// Get a list of connected voices with their metadata.
    pub fn voices(&self) -> Vec<VoiceInfo> {
        self.voices
            .values()
            .map(|v| VoiceInfo {
                id: v.id,
                name: v.name.clone(),
                subscription_strings: v.subscription_strings.clone(),
                connected_at: v.connected_at,
            })
            .collect()
    }

    /// Get the manifest for a specific voice.
    pub fn manifest(&self, voice_id: VoiceId) -> Option<&VoiceManifest> {
        self.manifests.get(&voice_id)
    }

    /// Get all manifests.
    pub fn manifests(&self) -> &HashMap<VoiceId, VoiceManifest> {
        &self.manifests
    }

    /// Get the current param state.
    pub fn param_state(&self) -> Vec<ParamInfo> {
        self.param_state
            .iter()
            .map(|(address, (source, msg))| {
                let source_name = self
                    .voices
                    .get(source)
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| format!("voice-{}", source));
                ParamInfo {
                    address: address.clone(),
                    source: *source,
                    source_name,
                    message: msg.clone(),
                }
            })
            .collect()
    }

    /// Get the scheduled actions.
    pub fn scheduled_actions(&self) -> Vec<ScheduledActionInfo> {
        self.schedule
            .values()
            .flat_map(|actions| {
                actions.iter().map(move |sa| {
                    let source_name = self
                        .voices
                        .get(&sa.source)
                        .map(|v| v.name.clone())
                        .unwrap_or_else(|| format!("voice-{}", sa.source));
                    ScheduledActionInfo {
                        timestamp: sa.timestamp,
                        address: sa.address.clone(),
                        source: sa.source,
                        source_name,
                        signal_type: sa.signal_type,
                    }
                })
            })
            .collect()
    }

    /// Get the event log.
    pub fn event_log(&self) -> &[String] {
        &self.event_log
    }

    /// Get the action log.
    pub fn action_log(&self) -> &VecDeque<ActionLogEntry> {
        &self.action_log
    }
}

impl Default for HubState {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about a connected voice (for TUI display).
#[derive(Clone)]
pub struct VoiceInfo {
    pub id: VoiceId,
    pub name: String,
    pub subscription_strings: Vec<String>,
    pub connected_at: f64,
}

/// Information about a param state entry (for TUI display).
#[derive(Clone)]
pub struct ParamInfo {
    pub address: String,
    pub source: VoiceId,
    pub source_name: String,
    pub message: WireMessage,
}

/// Information about a scheduled action (for TUI display).
#[derive(Clone)]
pub struct ScheduledActionInfo {
    pub timestamp: f64,
    pub address: String,
    pub source: VoiceId,
    pub source_name: String,
    pub signal_type: SignalType,
}

/// Shared state type alias.
pub type SharedState = Arc<Mutex<HubState>>;

// ---------------------------------------------------------------------------
// Payload extraction helpers
// ---------------------------------------------------------------------------

/// Extract the payload map from a WireMessage, returning an empty map if not a Map.
fn payload_map(msg: &WireMessage) -> BTreeMap<String, Value> {
    match &msg.payload {
        Value::Map(m) => m.clone(),
        _ => BTreeMap::new(),
    }
}

/// Parse a SignalType from its string representation.
fn parse_signal_type(s: &str) -> Option<SignalType> {
    match s {
        "event" => Some(SignalType::Event),
        "param" => Some(SignalType::Param),
        "stream" => Some(SignalType::Stream),
        _ => None,
    }
}

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
    let hello_msg = match codec::read_message(&mut reader).await {
        Ok(msg) if msg.msg_type == MSG_HELLO => msg,
        Ok(other) => {
            let mut st = state.lock().await;
            st.log(format!("[{peer}] Expected hello, got {:?}", other.msg_type));
            return;
        }
        Err(e) => {
            let mut st = state.lock().await;
            st.log(format!("[{peer}] Error reading hello: {e}"));
            return;
        }
    };

    // Extract hello fields.
    let hello_map = payload_map(&hello_msg);
    let protocol_version = get_integer(&hello_map, "protocol_version").unwrap_or(0) as u32;
    let voice_name = get_string(&hello_map, "name").unwrap_or_else(|| "unknown".into());

    // Check protocol version.
    if protocol_version != PROTOCOL_VERSION {
        let err_msg = error(
            ERR_UNSUPPORTED_PROTOCOL_VERSION,
            format!(
                "Hub supports protocol version {PROTOCOL_VERSION}, client requested {protocol_version}"
            ),
        );
        let _ = codec::write_message(&mut writer, &err_msg).await;
        let mut st = state.lock().await;
        st.log(format!(
            "[{peer}] Rejected: unsupported protocol version {protocol_version}"
        ));
        return;
    }

    // Register voice (no initial subscriptions — they subscribe after welcome).
    let (tx, mut rx) = mpsc::channel::<WireMessage>(256);
    let voice_id;
    {
        let mut st = state.lock().await;
        voice_id = st.next_voice_id;
        st.next_voice_id += 1;
        let connected_at = st.now();

        st.voices.insert(
            voice_id,
            ConnectedVoice {
                id: voice_id,
                name: voice_name.clone(),
                subscription_patterns: Vec::new(),
                subscription_strings: Vec::new(),
                tx: tx.clone(),
                connected_at,
            },
        );

        st.log(format!(
            "Voice {voice_id} connected: \"{voice_name}\" from {peer}"
        ));

        // Send Welcome.
        let welcome_msg = welcome(voice_id);
        if let Err(e) = codec::write_message(&mut writer, &welcome_msg).await {
            st.log(format!("Voice {voice_id}: failed to send welcome: {e}"));
            st.voices.remove(&voice_id);
            return;
        }

        // Emit hub event: voice joined.
        let joined_payload = Value::Map({
            let mut m = BTreeMap::new();
            m.insert("voice_id".into(), Value::Integer(voice_id as i64));
            m.insert("name".into(), Value::String(voice_name.clone()));
            m
        });
        emit_hub_event(&st, "/hub/voice/joined", joined_payload).await;

        // Replay current param state to the new voice (no subscriptions yet,
        // so this will be empty until the voice subscribes — but we keep the
        // logic for when subscriptions are added post-welcome).
        let patterns: Vec<Pattern> = st
            .voices
            .get(&voice_id)
            .map(|v| v.subscription_patterns.clone())
            .unwrap_or_default();
        for (_source, action_msg) in st.param_state.values() {
            let action_map = payload_map(action_msg);
            let address = get_string(&action_map, "address").unwrap_or_default();
            if matches_any(&patterns, &address) {
                let _ = tx.send(action_msg.clone()).await;
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
            Ok(msg) => {
                match msg.msg_type.as_str() {
                    MSG_CLOCK_PING => {
                        let map = payload_map(&msg);
                        let sequence = get_integer(&map, "sequence").unwrap_or(0) as u64;
                        let st = state.lock().await;
                        let hub_time = st.now();
                        let pong = clock_pong(sequence, hub_time);
                        let _ = tx.send(pong).await;
                    }

                    MSG_ACTION => {
                        let map = payload_map(&msg);
                        let address = get_string(&map, "address").unwrap_or_default();
                        let signal_type = get_string(&map, "signal_type")
                            .and_then(|s| parse_signal_type(&s))
                            .unwrap_or(SignalType::Event);
                        let timestamp = get_float(&map, "timestamp").unwrap_or(0.0);
                        let payload = get_value(&map, "payload").unwrap_or(Value::Null);

                        // Reserved namespace enforcement: reject actions to /hub/**.
                        if address.starts_with("/hub/") {
                            let err_msg = error(
                                ERR_RESERVED_NAMESPACE,
                                format!("Address '{address}' is in the reserved /hub/ namespace"),
                            );
                            let _ = tx.send(err_msg).await;
                            continue;
                        }

                        // Create action with source set for routing.
                        let routed_msg = action_with_source(
                            voice_id,
                            address.clone(),
                            signal_type,
                            timestamp,
                            payload,
                        );

                        let mut st = state.lock().await;
                        let now = st.now();

                        // If the action has a future timestamp, schedule it.
                        if timestamp > 0.0 && timestamp > now {
                            // For future params, do NOT write to param_state yet.
                            // Activation-time retention: param becomes current only at its timestamp.
                            let key = timestamp_key(timestamp);
                            st.schedule.entry(key).or_default().push(ScheduledAction {
                                source: voice_id,
                                message: routed_msg,
                                address,
                                signal_type,
                                timestamp,
                            });
                        } else {
                            // Immediate dispatch: store param state now.
                            if signal_type == SignalType::Param {
                                st.param_state
                                    .insert(address.clone(), (voice_id, routed_msg.clone()));
                            }
                            // Log the action for the Action Monitor.
                            let source_name = st
                                .voices
                                .get(&voice_id)
                                .map(|v| v.name.clone())
                                .unwrap_or_else(|| format!("voice-{}", voice_id));
                            st.log_action(ActionLogEntry {
                                timestamp: now,
                                source: voice_id,
                                source_name,
                                address: address.clone(),
                                signal_type,
                            });
                            // Route immediately.
                            route_action(&st, voice_id, &address, &routed_msg).await;
                        }
                    }

                    MSG_UNSET_PARAM => {
                        let map = payload_map(&msg);
                        let address = get_string(&map, "address").unwrap_or_default();
                        let mut st = state.lock().await;
                        st.param_state.remove(&address);
                        // Broadcast unset to all subscribers of this address.
                        for voice in st.voices.values() {
                            if voice.id == voice_id {
                                continue;
                            }
                            if matches_any(&voice.subscription_patterns, &address) {
                                let _ = voice.tx.send(msg.clone()).await;
                            }
                        }
                    }

                    MSG_SUBSCRIBE => {
                        let map = payload_map(&msg);
                        let pat_str = get_string(&map, "pattern").unwrap_or_default();
                        let mut st = state.lock().await;
                        match Pattern::parse(&pat_str) {
                            Ok(p) => {
                                // Collect matching param replays before mutating voice.
                                let mut replays = Vec::new();
                                if let Some(voice) = st.voices.get(&voice_id) {
                                    let mut patterns = voice.subscription_patterns.clone();
                                    patterns.push(p.clone());
                                    for (_source, action_msg) in st.param_state.values() {
                                        let action_map = payload_map(action_msg);
                                        let address =
                                            get_string(&action_map, "address").unwrap_or_default();
                                        if matches_any(&patterns, &address) {
                                            replays.push(action_msg.clone());
                                        }
                                    }
                                }
                                // Now mutate voice and send replays.
                                if let Some(voice) = st.voices.get_mut(&voice_id) {
                                    voice.subscription_patterns.push(p);
                                    voice.subscription_strings.push(pat_str.clone());
                                    for action_msg in replays {
                                        let _ = voice.tx.send(action_msg).await;
                                    }
                                }
                            }
                            Err(e) => {
                                let err_msg = error(
                                    ERR_INVALID_PATTERN,
                                    format!("Invalid subscribe pattern '{pat_str}': {e}"),
                                );
                                if let Some(voice) = st.voices.get(&voice_id) {
                                    let _ = voice.tx.send(err_msg).await;
                                }
                                st.log(format!(
                                    "Voice {voice_id}: invalid subscribe pattern '{pat_str}': {e}"
                                ));
                            }
                        }
                    }

                    MSG_UNSUBSCRIBE => {
                        let map = payload_map(&msg);
                        let pat_str = get_string(&map, "pattern").unwrap_or_default();
                        let mut st = state.lock().await;
                        if let Some(voice) = st.voices.get_mut(&voice_id) {
                            // Remove from string list and parsed patterns in tandem.
                            if let Some(pos) = voice
                                .subscription_strings
                                .iter()
                                .position(|s| s == &pat_str)
                            {
                                voice.subscription_strings.remove(pos);
                                voice.subscription_patterns.remove(pos);
                            }
                        }
                    }

                    MSG_DISCONNECT => {
                        let mut st = state.lock().await;
                        if let Some(name) = st.remove_voice(voice_id, "disconnect") {
                            // Emit hub event: voice left.
                            let left_payload = Value::Map({
                                let mut m = BTreeMap::new();
                                m.insert("voice_id".into(), Value::Integer(voice_id as i64));
                                m.insert("name".into(), Value::String(name));
                                m
                            });
                            emit_hub_event(&st, "/hub/voice/left", left_payload).await;
                        }
                        break;
                    }

                    MSG_UPDATE_NAME => {
                        let map = payload_map(&msg);
                        let new_name = get_string(&map, "name").unwrap_or_default();
                        let mut st = state.lock().await;
                        let old_name = if let Some(voice) = st.voices.get_mut(&voice_id) {
                            let old = voice.name.clone();
                            voice.name = new_name.clone();
                            Some(old)
                        } else {
                            None
                        };
                        if let Some(old) = old_name {
                            st.log(format!(
                                "Voice {voice_id} renamed: \"{old}\" -> \"{new_name}\""
                            ));
                            // Emit hub event: voice renamed.
                            let renamed_payload = Value::Map({
                                let mut m = BTreeMap::new();
                                m.insert("voice_id".into(), Value::Integer(voice_id as i64));
                                m.insert("old_name".into(), Value::String(old));
                                m.insert("new_name".into(), Value::String(new_name));
                                m
                            });
                            emit_hub_event(&st, "/hub/voice/renamed", renamed_payload).await;
                        }
                    }

                    MSG_SET_MANIFEST => {
                        let map = payload_map(&msg);
                        let manifest_value = get_value(&map, "manifest").unwrap_or(Value::Null);
                        let mut st = state.lock().await;
                        match VoiceManifest::from_value(&manifest_value) {
                            Some(manifest) => {
                                st.log(format!(
                                    "Voice {voice_id}: manifest set (\"{}\")",
                                    manifest.name
                                ));
                                st.manifests.insert(voice_id, manifest);
                                // Emit hub event: manifest set.
                                let set_payload = Value::Map({
                                    let mut m = BTreeMap::new();
                                    m.insert("voice_id".into(), Value::Integer(voice_id as i64));
                                    m.insert("manifest".into(), manifest_value.clone());
                                    m
                                });
                                emit_hub_event(&st, "/hub/manifest/set", set_payload).await;
                            }
                            None => {
                                let err_msg = error(
                                    ERR_MALFORMED_MANIFEST,
                                    "set_manifest: manifest is not a valid manifest map",
                                );
                                if let Some(voice) = st.voices.get(&voice_id) {
                                    let _ = voice.tx.send(err_msg).await;
                                }
                                st.log(format!(
                                    "Voice {voice_id}: malformed set_manifest rejected"
                                ));
                            }
                        }
                    }

                    MSG_PATCH_MANIFEST => {
                        let map = payload_map(&msg);
                        let patch_value = get_value(&map, "patch").unwrap_or(Value::Null);
                        let mut st = state.lock().await;
                        let patch_map = match &patch_value {
                            Value::Map(m) => m.clone(),
                            _ => {
                                let err_msg = error(
                                    ERR_MALFORMED_MANIFEST,
                                    "patch_manifest: patch must be a map",
                                );
                                if let Some(voice) = st.voices.get(&voice_id) {
                                    let _ = voice.tx.send(err_msg).await;
                                }
                                st.log(format!(
                                    "Voice {voice_id}: malformed patch_manifest rejected (not a map)"
                                ));
                                return;
                            }
                        };
                        // Get or create the manifest, apply the patch, and capture the name.
                        let name = {
                            let manifest = st
                                .manifests
                                .entry(voice_id)
                                .or_insert_with(VoiceManifest::default);
                            manifest.apply_patch(&patch_map);
                            manifest.name.clone()
                        };
                        st.log(format!("Voice {voice_id}: manifest patched (\"{name}\")"));
                        // Emit hub event: manifest updated.
                        let updated_payload = Value::Map({
                            let mut m = BTreeMap::new();
                            m.insert("voice_id".into(), Value::Integer(voice_id as i64));
                            m.insert("patch".into(), patch_value);
                            m
                        });
                        emit_hub_event(&st, "/hub/manifest/updated", updated_payload).await;
                    }

                    other => {
                        let mut st = state.lock().await;
                        st.log(format!(
                            "Voice {voice_id}: unexpected message type '{other}'"
                        ));
                    }
                }
            }

            Err(CodecError::ConnectionClosed) => {
                let mut st = state.lock().await;
                if let Some(name) = st.remove_voice(voice_id, "connection closed") {
                    // Emit hub event: voice left.
                    let left_payload = Value::Map({
                        let mut m = BTreeMap::new();
                        m.insert("voice_id".into(), Value::Integer(voice_id as i64));
                        m.insert("name".into(), Value::String(name));
                        m
                    });
                    emit_hub_event(&st, "/hub/voice/left", left_payload).await;
                }
                break;
            }

            Err(e) => {
                let mut st = state.lock().await;
                if let Some(name) = st.remove_voice(voice_id, &format!("read error: {e}")) {
                    // Emit hub event: voice left.
                    let left_payload = Value::Map({
                        let mut m = BTreeMap::new();
                        m.insert("voice_id".into(), Value::Integer(voice_id as i64));
                        m.insert("name".into(), Value::String(name));
                        m
                    });
                    emit_hub_event(&st, "/hub/voice/left", left_payload).await;
                }
                break;
            }
        }
    }

    writer_handle.abort();
}

/// Emit a hub event as an ordinary action with source=0 (hub).
/// Hub events are routed to all subscribers of /hub/** addresses.
/// They do not alter protocol state.
async fn emit_hub_event(st: &HubState, address: &str, payload: Value) {
    let now = st.now();
    let msg = action_with_source(0, address, SignalType::Event, now, payload);
    // Route to all subscribers of /hub/** (source=0 means no exclusion).
    for voice in st.voices.values() {
        if matches_any(&voice.subscription_patterns, address) {
            // Hub events are guaranteed delivery (not streams).
            let _ = voice.tx.send(msg.clone()).await;
        }
    }
}

/// Route an action to all subscribed voices (except the sender).
/// Streams are dropped if the channel is full (best-effort delivery).
/// Events and params wait for channel capacity (guaranteed delivery).
async fn route_action(st: &HubState, source: VoiceId, address: &str, msg: &WireMessage) {
    // Extract signal type from the message for congestion handling.
    let signal_type = match &msg.payload {
        Value::Map(m) => get_string(m, "signal_type")
            .and_then(|s| parse_signal_type(&s))
            .unwrap_or(SignalType::Event),
        _ => SignalType::Event,
    };

    for voice in st.voices.values() {
        if voice.id == source {
            continue;
        }
        if matches_any(&voice.subscription_patterns, address) {
            if signal_type == SignalType::Stream {
                // Streams are best-effort: drop if channel is full.
                let _ = voice.tx.try_send(msg.clone());
            } else {
                // Events and params are guaranteed: wait for capacity.
                let _ = voice.tx.send(msg.clone()).await;
            }
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
            let due_keys: Vec<u64> = st.schedule.range(..=now_key).map(|(k, _)| *k).collect();

            // Dispatch them.
            for key in due_keys {
                if let Some(actions) = st.schedule.remove(&key) {
                    for scheduled in actions {
                        // Activation-time retention: activate param state now.
                        if scheduled.signal_type == SignalType::Param {
                            st.param_state.insert(
                                scheduled.address.clone(),
                                (scheduled.source, scheduled.message.clone()),
                            );
                        }
                        // Log the action for the Action Monitor.
                        let source_name = st
                            .voices
                            .get(&scheduled.source)
                            .map(|v| v.name.clone())
                            .unwrap_or_else(|| format!("voice-{}", scheduled.source));
                        st.log_action(ActionLogEntry {
                            timestamp: now,
                            source: scheduled.source,
                            source_name,
                            address: scheduled.address.clone(),
                            signal_type: scheduled.signal_type,
                        });
                        route_action(
                            &st,
                            scheduled.source,
                            &scheduled.address,
                            &scheduled.message,
                        )
                        .await;
                    }
                }
            }
        }
        // Poll every 1ms for tight scheduling.
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Start the hub's TCP accept loop. Returns the shared state and actual port bound to.
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
