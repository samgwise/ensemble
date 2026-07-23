//! MIDI key state tracker with mutex counter for retrigger/cancel safety.
//!
//! Each channel/note pair has an event counter (mutex). When a new play or
//! cancel arrives, the counter is bumped. A pending note-off only fires if
//! its event ID still matches the current counter — otherwise, a newer event
//! has superseded it and the note-off is silently dropped.

const NOTE_ON: u8 = 0x90;
const NOTE_OFF: u8 = 0x80;

/// A raw 3-byte MIDI message ready to send to the output port.
pub struct MidiBytes(pub [u8; 3]);

/// State for a single channel/note pair.
#[derive(Debug, Clone, Copy)]
struct KeyState {
    /// Monotonically increasing event counter.
    event_counter: u32,
    /// Whether the key is currently sounding.
    key_on: bool,
}

impl KeyState {
    fn new() -> Self {
        Self {
            event_counter: 0,
            key_on: false,
        }
    }

    /// Bump the mutex counter and return the new value.
    fn bump(&mut self) -> u32 {
        self.event_counter = self.event_counter.wrapping_add(1);
        self.event_counter
    }

    /// Current mutex counter value.
    fn current(&self) -> u32 {
        self.event_counter
    }

    /// Attempt to play. Only succeeds if `event_id` matches the current
    /// counter and the key is not already on.
    fn play(&mut self, event_id: u32, channel: u8, note: u8, velocity: u8) -> Option<MidiBytes> {
        if self.event_counter == event_id && !self.key_on {
            self.key_on = true;
            Some(MidiBytes([NOTE_ON | channel, note, velocity]))
        } else {
            None
        }
    }

    /// Attempt to stop. Only succeeds if `event_id` matches the current
    /// counter and the key is on.
    fn stop(&mut self, event_id: u32, channel: u8, note: u8) -> Option<MidiBytes> {
        if self.event_counter == event_id && self.key_on {
            self.key_on = false;
            Some(MidiBytes([NOTE_OFF | channel, note, 0]))
        } else {
            None
        }
    }
}

/// Holds the key state for all 16 MIDI channels x 128 notes.
pub struct KeyStateStore {
    channels: [[KeyState; 128]; 16],
}

impl KeyStateStore {
    pub fn new() -> Self {
        Self {
            channels: [[KeyState::new(); 128]; 16],
        }
    }

    fn key(&mut self, channel: u8, note: u8) -> &mut KeyState {
        &mut self.channels[channel as usize][note as usize]
    }

    /// Bump the mutex for a channel/note and return the new event ID.
    /// Call this when scheduling a new play or cancel.
    pub fn bump(&mut self, channel: u8, note: u8) -> u32 {
        self.key(channel, note).bump()
    }

    /// Get the current event ID without bumping.
    pub fn current(&mut self, channel: u8, note: u8) -> u32 {
        self.key(channel, note).current()
    }

    /// Attempt note-on. Returns MIDI bytes if the event ID is still valid.
    pub fn play(
        &mut self,
        event_id: u32,
        channel: u8,
        note: u8,
        velocity: u8,
    ) -> Option<MidiBytes> {
        self.key(channel, note)
            .play(event_id, channel, note, velocity)
    }

    /// Attempt note-off. Returns MIDI bytes if the event ID is still valid.
    pub fn stop(&mut self, event_id: u32, channel: u8, note: u8) -> Option<MidiBytes> {
        self.key(channel, note).stop(event_id, channel, note)
    }
}
