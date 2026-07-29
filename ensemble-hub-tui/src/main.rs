//! Ensemble Hub TUI — terminal user interface for the Ensemble hub.
//!
//! Provides a rich terminal interface for monitoring and debugging Ensemble systems.

use std::collections::HashMap;
use std::io;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs},
    Frame, Terminal,
};

use ensemble_core::protocol::*;
use ensemble_discovery::{delete_port_file, is_port_bound, read_port_file, write_port_file};
use ensemble_hub::{
    start_server, ActionLogEntry, HubState, ParamInfo, ScheduledActionInfo, SharedState, VoiceInfo,
};
use ensemble_routing::Pattern;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// Which detail pane is currently displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailPane {
    Params,
    Schedule,
    Log,
    Manifest,
    RouteTester,
}

/// Application state for the TUI.
struct App {
    /// Index of the selected voice in the Voice Browser.
    voice_selection: usize,
    /// Which detail pane is active.
    detail_pane: DetailPane,
    /// Route tester pattern input.
    route_pattern: String,
    /// Route tester address input.
    route_address: String,
    /// Which input field is focused in Route Tester (0 = pattern, 1 = address).
    route_input_focus: usize,
    /// Whether we're in input mode for the route tester.
    route_input_mode: bool,
    /// Should the app quit?
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            voice_selection: 0,
            detail_pane: DetailPane::Params,
            route_pattern: String::new(),
            route_address: String::new(),
            route_input_focus: 0,
            route_input_mode: false,
            should_quit: false,
        }
    }
}

// ---------------------------------------------------------------------------
// State snapshot
// ---------------------------------------------------------------------------

/// A point-in-time copy of everything the TUI draws.
///
/// Captured under the `HubState` lock so the lock is never held across
/// `terminal.draw()`, which can be slow and would otherwise stall the hub.
struct StateSnapshot {
    /// Hub time at capture.
    now: f64,
    /// Connected voices, sorted by ID for a stable display order.
    voices: Vec<VoiceInfo>,
    /// Manifests by voice ID.
    manifests: HashMap<VoiceId, VoiceManifest>,
    /// Current param state.
    params: Vec<ParamInfo>,
    /// Scheduled actions.
    scheduled: Vec<ScheduledActionInfo>,
    /// Recent routed actions (oldest first).
    action_log: Vec<ActionLogEntry>,
    /// Recent hub events (oldest first).
    event_log: Vec<String>,
}

impl StateSnapshot {
    /// Clone everything the TUI needs out of the hub state.
    fn capture(state: &HubState) -> Self {
        // HashMap iteration order is arbitrary, so sort voices by ID to keep
        // the Voice Browser (and the meaning of the selection index) stable.
        let mut voices = state.voices();
        voices.sort_by_key(|v| v.id);
        Self {
            now: state.now(),
            voices,
            manifests: state.manifests().clone(),
            params: state.param_state(),
            scheduled: state.scheduled_actions(),
            action_log: state.action_log().iter().cloned().collect(),
            event_log: state.event_log().to_vec(),
        }
    }
}

/// Clamp a selection index to a list of `len` items (an empty list clamps
/// to 0). Guards against voices disconnecting between draws.
fn clamp_selection(selection: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        selection.min(len - 1)
    }
}

/// Truncate a payload string for display.
///
/// Operates on chars rather than bytes so multi-byte UTF-8 content can
/// never cause a slice panic.
fn truncate_payload(payload: &str) -> String {
    if payload.chars().count() > 40 {
        format!("{}...", payload.chars().take(37).collect::<String>())
    } else {
        payload.to_string()
    }
}

// ---------------------------------------------------------------------------
// TUI rendering
// ---------------------------------------------------------------------------

/// Draw the entire TUI.
fn draw(frame: &mut Frame, app: &App, snap: &StateSnapshot) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),      // Header
            Constraint::Length(3),      // Tabs
            Constraint::Percentage(40), // Top: Voices + Manifest
            Constraint::Percentage(30), // Middle: Action Monitor
            Constraint::Percentage(30), // Bottom: Details
        ])
        .split(frame.area());

    draw_header(frame, snap, chunks[0]);
    draw_tabs(frame, app, chunks[1]);

    // Top section: Voices + Manifest side by side.
    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[2]);

    draw_voice_browser(frame, app, snap, top_chunks[0]);
    draw_manifest_browser(frame, app, snap, top_chunks[1]);

    draw_action_monitor(frame, snap, chunks[3]);
    draw_detail_pane(frame, app, snap, chunks[4]);
}

/// Draw the header bar.
fn draw_header(frame: &mut Frame, snap: &StateSnapshot, area: Rect) {
    let hub_time = snap.now;
    let voice_count = snap.voices.len();
    let scheduled = snap.scheduled.len();

    let header = Paragraph::new(format!(
        " Ensemble Hub | time: {hub_time:.2}s | voices: {voice_count} | scheduled: {scheduled} | 'q' quit, 'Tab' cycle detail, '1-5' select detail"
    ))
    .style(Style::default().fg(Color::Cyan))
    .block(Block::default().borders(Borders::ALL).title(" Ensemble Hub "));
    frame.render_widget(header, area);
}

/// Draw the tab bar for detail pane selection.
fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles = vec![
        Line::from("1:Params"),
        Line::from("2:Schedule"),
        Line::from("3:Log"),
        Line::from("4:Manifest"),
        Line::from("5:Route Tester"),
    ];

    let selected = match app.detail_pane {
        DetailPane::Params => 0,
        DetailPane::Schedule => 1,
        DetailPane::Log => 2,
        DetailPane::Manifest => 3,
        DetailPane::RouteTester => 4,
    };

    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Detail Pane "),
        );
    frame.render_widget(tabs, area);
}

/// Draw the Voice Browser pane.
fn draw_voice_browser(frame: &mut Frame, app: &App, snap: &StateSnapshot, area: Rect) {
    let voices = &snap.voices;
    let items: Vec<ListItem> = voices
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let style = if i == app.voice_selection {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let connected_secs = snap.now - v.connected_at;
            ListItem::new(format!(
                "  #{}: \"{}\"  ({:.0}s ago)",
                v.id, v.name, connected_secs
            ))
            .style(style)
        })
        .collect();

    let voices_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Voice Browser ({}) ", voices.len())),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    frame.render_widget(voices_list, area);
}

/// Draw the Manifest Browser pane for the selected voice.
fn draw_manifest_browser(frame: &mut Frame, app: &App, snap: &StateSnapshot, area: Rect) {
    let content = if let Some(voice) = snap.voices.get(app.voice_selection) {
        if let Some(manifest) = snap.manifests.get(&voice.id) {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("Name: ", Style::default().fg(Color::Green)),
                    Span::raw(&manifest.name),
                ]),
                Line::from(""),
            ];

            if let Some(desc) = &manifest.description {
                lines.push(Line::from(vec![
                    Span::styled("Description: ", Style::default().fg(Color::Green)),
                    Span::raw(desc),
                ]));
            }

            if let Some(ver) = &manifest.version {
                lines.push(Line::from(vec![
                    Span::styled("Version: ", Style::default().fg(Color::Green)),
                    Span::raw(ver),
                ]));
            }

            if !manifest.tags.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Tags: ", Style::default().fg(Color::Green)),
                    Span::raw(manifest.tags.join(", ")),
                ]));
            }

            if !manifest.provides.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Provides: ", Style::default().fg(Color::Green)),
                    Span::raw(manifest.provides.join(", ")),
                ]));
            }

            if !manifest.expects.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Expects: ", Style::default().fg(Color::Green)),
                    Span::raw(manifest.expects.join(", ")),
                ]));
            }

            if !manifest.routes.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Routes:",
                    Style::default().fg(Color::Green),
                )));
                for route in &manifest.routes {
                    let desc = route.description.as_deref().unwrap_or("");
                    lines.push(Line::from(format!("  {} -> {}", route.address, desc)));
                }
            }

            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Manifest: {} ", voice.name)),
            )
        } else {
            Paragraph::new("No manifest set for this voice.").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Manifest: {} ", voice.name)),
            )
        }
    } else {
        Paragraph::new("No voice selected.").block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Manifest Browser "),
        )
    };

    frame.render_widget(content, area);
}

/// Draw the Action Monitor pane.
fn draw_action_monitor(frame: &mut Frame, snap: &StateSnapshot, area: Rect) {
    let action_log = &snap.action_log;
    let items: Vec<ListItem> = action_log
        .iter()
        .rev()
        .take(20)
        .map(|entry| {
            let signal_str = match entry.signal_type {
                SignalType::Event => "EVT",
                SignalType::Param => "PAR",
                SignalType::Stream => "STR",
            };
            ListItem::new(format!(
                "  [{:.3}] {} {} -> {}",
                entry.timestamp, signal_str, entry.source_name, entry.address
            ))
        })
        .collect();

    let action_list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Action Monitor ({} total) ", action_log.len())),
    );

    frame.render_widget(action_list, area);
}

/// Draw the detail pane based on current selection.
fn draw_detail_pane(frame: &mut Frame, app: &App, snap: &StateSnapshot, area: Rect) {
    match app.detail_pane {
        DetailPane::Params => draw_param_inspector(frame, app, snap, area),
        DetailPane::Schedule => draw_schedule_monitor(frame, snap, area),
        DetailPane::Log => draw_log_viewer(frame, snap, area),
        DetailPane::Manifest => draw_manifest_detail(frame, app, snap, area),
        DetailPane::RouteTester => draw_route_tester(frame, app, area),
    }
}

/// Draw the Param Inspector.
fn draw_param_inspector(frame: &mut Frame, app: &App, snap: &StateSnapshot, area: Rect) {
    let selected_voice_id = snap.voices.get(app.voice_selection).map(|v| v.id);

    let params: Vec<&ParamInfo> = snap
        .params
        .iter()
        .filter(|p| selected_voice_id.is_none_or(|vid| p.source == vid))
        .collect();

    let items: Vec<ListItem> = params
        .iter()
        .map(|p| {
            let payload_str = match &p.message.payload {
                Value::Map(m) => {
                    if let Some(v) = m.get("payload") {
                        format!("{:?}", v)
                    } else {
                        "?".to_string()
                    }
                }
                _ => format!("{:?}", p.message.payload),
            };
            // Truncate long payloads (char-safe for multi-byte UTF-8).
            let payload_display = truncate_payload(&payload_str);
            ListItem::new(format!(
                "  {} = {}  (from {})",
                p.address, payload_display, p.source_name
            ))
        })
        .collect();

    let param_list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Param Inspector ({}) ", params.len())),
    );

    frame.render_widget(param_list, area);
}

/// Draw the Scheduling Monitor.
fn draw_schedule_monitor(frame: &mut Frame, snap: &StateSnapshot, area: Rect) {
    let scheduled = &snap.scheduled;
    let items: Vec<ListItem> = scheduled
        .iter()
        .map(|sa| {
            let signal_str = match sa.signal_type {
                SignalType::Event => "EVT",
                SignalType::Param => "PAR",
                SignalType::Stream => "STR",
            };
            let now = snap.now;
            let remaining = (sa.timestamp - now).max(0.0);
            ListItem::new(format!(
                "  [{:.3}] {} {} -> {} (in {:.2}s)",
                sa.timestamp, signal_str, sa.source_name, sa.address, remaining
            ))
        })
        .collect();

    let schedule_list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Scheduling Monitor ({}) ", scheduled.len())),
    );

    frame.render_widget(schedule_list, area);
}

/// Draw the Log Viewer.
fn draw_log_viewer(frame: &mut Frame, snap: &StateSnapshot, area: Rect) {
    let event_log = &snap.event_log;
    let items: Vec<ListItem> = event_log
        .iter()
        .rev()
        .take(20)
        .map(|msg| ListItem::new(format!("  {}", msg)))
        .collect();

    let log_list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Log Viewer ({}) ", event_log.len())),
    );

    frame.render_widget(log_list, area);
}

/// Draw the Manifest detail view (same as manifest browser but in detail pane).
fn draw_manifest_detail(frame: &mut Frame, app: &App, snap: &StateSnapshot, area: Rect) {
    draw_manifest_browser(frame, app, snap, area);
}

/// Draw the Route Tester.
fn draw_route_tester(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Pattern: ", Style::default().fg(Color::Green)),
            if app.route_input_focus == 0 && app.route_input_mode {
                Span::styled(
                    format!("{}_", app.route_pattern),
                    Style::default().fg(Color::Yellow),
                )
            } else {
                Span::raw(&app.route_pattern)
            },
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Address: ", Style::default().fg(Color::Green)),
            if app.route_input_focus == 1 && app.route_input_mode {
                Span::styled(
                    format!("{}_", app.route_address),
                    Style::default().fg(Color::Yellow),
                )
            } else {
                Span::raw(&app.route_address)
            },
        ]),
        Line::from(""),
    ];

    // Show match result if both pattern and address are non-empty.
    if !app.route_pattern.is_empty() && !app.route_address.is_empty() {
        match Pattern::parse(&app.route_pattern) {
            Ok(pattern) => {
                if ensemble_routing::matches_any(&[pattern], &app.route_address) {
                    lines.push(Line::from(vec![
                        Span::styled("  Result: ", Style::default().fg(Color::Green)),
                        Span::styled(
                            "MATCH",
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("  Result: ", Style::default().fg(Color::Red)),
                        Span::styled(
                            "NO MATCH",
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                }
            }
            Err(e) => {
                lines.push(Line::from(vec![
                    Span::styled("  Error: ", Style::default().fg(Color::Red)),
                    Span::raw(format!("Invalid pattern: {}", e)),
                ]));
            }
        }
    } else {
        lines.push(Line::from(
            "  Press 'i' to enter input mode, then type pattern and address.",
        ));
        lines.push(Line::from(
            "  Press 'Tab' to switch between fields, 'Esc' to exit input mode.",
        ));
    }

    let input_mode_label = if app.route_input_mode {
        " [INPUT MODE]"
    } else {
        ""
    };

    let route_tester = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Route Tester{} ", input_mode_label)),
    );

    frame.render_widget(route_tester, area);
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

/// Handle keyboard input.
fn handle_input(app: &mut App, key: event::KeyEvent) {
    if key.kind != KeyEventKind::Press {
        return;
    }

    // If in route tester input mode, handle text input.
    if app.route_input_mode && app.detail_pane == DetailPane::RouteTester {
        match key.code {
            KeyCode::Esc => {
                app.route_input_mode = false;
            }
            KeyCode::Tab => {
                // Switch between pattern and address fields.
                app.route_input_focus = if app.route_input_focus == 0 { 1 } else { 0 };
            }
            KeyCode::Enter => {
                // Exit input mode on Enter.
                app.route_input_mode = false;
            }
            KeyCode::Backspace => {
                if app.route_input_focus == 0 {
                    app.route_pattern.pop();
                } else {
                    app.route_address.pop();
                }
            }
            KeyCode::Char(c) => {
                if app.route_input_focus == 0 {
                    app.route_pattern.push(c);
                } else {
                    app.route_address.push(c);
                }
            }
            _ => {}
        }
        return;
    }

    // Normal mode.
    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Tab => {
            // Cycle detail pane.
            app.detail_pane = match app.detail_pane {
                DetailPane::Params => DetailPane::Schedule,
                DetailPane::Schedule => DetailPane::Log,
                DetailPane::Log => DetailPane::Manifest,
                DetailPane::Manifest => DetailPane::RouteTester,
                DetailPane::RouteTester => DetailPane::Params,
            };
        }
        KeyCode::Char('1') => app.detail_pane = DetailPane::Params,
        KeyCode::Char('2') => app.detail_pane = DetailPane::Schedule,
        KeyCode::Char('3') => app.detail_pane = DetailPane::Log,
        KeyCode::Char('4') => app.detail_pane = DetailPane::Manifest,
        KeyCode::Char('5') => app.detail_pane = DetailPane::RouteTester,
        KeyCode::Up | KeyCode::Char('k') => {
            if app.voice_selection > 0 {
                app.voice_selection -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.voice_selection += 1;
        }
        KeyCode::Char('i') if app.detail_pane == DetailPane::RouteTester => {
            app.route_input_mode = true;
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Main TUI loop
// ---------------------------------------------------------------------------

/// RAII guard that restores the terminal on drop.
///
/// Ensures raw mode is disabled and the alternate screen is left even when
/// the TUI exits via an error or a panic, so the user's terminal is never
/// stranded.
struct TerminalGuard;

impl TerminalGuard {
    /// Enter raw mode and switch to the alternate screen.
    fn new() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Run the TUI.
async fn run_tui(state: SharedState) -> anyhow::Result<()> {
    // Set up the terminal; the guard restores it on every exit path.
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    loop {
        // Snapshot the hub state under the lock, then drop the guard before
        // drawing so the lock is never held across terminal I/O.
        let snap = {
            let st = state.lock().await;
            StateSnapshot::capture(&st)
        };

        // Keep the selection within the voice list as voices come and go.
        app.voice_selection = clamp_selection(app.voice_selection, snap.voices.len());

        terminal.draw(|frame| draw(frame, &app, &snap))?;

        // Handle input (non-blocking, 50ms timeout for responsive UI).
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                handle_input(&mut app, key);
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

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
    let (state, actual_port) = start_server(port).await?;

    // Publish the actual bound port so clients can discover us.
    write_port_file(actual_port)?;

    eprintln!("Ensemble Hub TUI starting on 127.0.0.1:{actual_port}");

    // Run the TUI; ensure the port file is removed on both success and error.
    let result = run_tui(state).await;
    let _ = delete_port_file();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_payload_short() {
        assert_eq!(truncate_payload("hello"), "hello");
    }

    #[test]
    fn test_truncate_payload_long_ascii() {
        let long = "a".repeat(50);
        let out = truncate_payload(&long);
        assert_eq!(out, format!("{}...", "a".repeat(37)));
    }

    #[test]
    fn test_truncate_payload_multibyte() {
        // 50 multi-byte chars: byte slicing at 37 would panic mid-character,
        // char-based truncation must not.
        let multi = "é".repeat(50);
        let out = truncate_payload(&multi);
        assert_eq!(out, format!("{}...", "é".repeat(37)));
        assert_eq!(out.chars().count(), 40);
    }

    #[test]
    fn test_truncate_payload_boundary() {
        // Exactly 40 chars is left alone; 41 is truncated.
        let forty = "x".repeat(40);
        assert_eq!(truncate_payload(&forty), forty);
        let forty_one = "x".repeat(41);
        assert_eq!(truncate_payload(&forty_one).chars().count(), 40);
    }

    #[test]
    fn test_clamp_selection() {
        assert_eq!(clamp_selection(0, 0), 0);
        assert_eq!(clamp_selection(5, 0), 0);
        assert_eq!(clamp_selection(2, 5), 2);
        assert_eq!(clamp_selection(5, 3), 2);
        assert_eq!(clamp_selection(0, 1), 0);
    }
}
