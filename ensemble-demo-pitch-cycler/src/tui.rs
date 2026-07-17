//! TUI — terminal user interface for the pitch pattern cycler.
//!
//! Provides real-time visualisation of the pitch pattern and keyboard controls
//! for editing the pattern, trigger address, output address, and MIDI params.

use std::io;
use std::sync::Arc;

use anyhow::Result;
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
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use tokio::sync::Mutex;

use crate::AppState;

/// MIDI note number to note name (e.g., 60 → "C4").
fn midi_note_name(note: i64) -> String {
    let names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let octave = (note / 12) - 1;
    let name = names[(note % 12) as usize];
    format!("{}{}", name, octave)
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Draw the entire TUI.
fn draw(frame: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(5), // Pattern
            Constraint::Length(6), // Params
            Constraint::Min(3),   // Controls
        ])
        .split(frame.area());

    draw_header(frame, state, chunks[0]);
    draw_pattern(frame, state, chunks[1]);
    draw_params(frame, state, chunks[2]);
    draw_controls(frame, chunks[3]);
}

/// Draw the header bar.
fn draw_header(frame: &mut Frame, state: &AppState, area: Rect) {
    let header = Paragraph::new(format!(
        " Pitch Cycler | Voice #{} | Index: {}/{}",
        state.hub.voice_id,
        state.current_index + 1,
        state.pattern.len()
    ))
    .style(Style::default().fg(Color::Cyan))
    .block(Block::default().borders(Borders::ALL).title(" Ensemble — Pitch Cycler "));
    frame.render_widget(header, area);
}

/// Draw the pattern visualization.
fn draw_pattern(frame: &mut Frame, state: &AppState, area: Rect) {
    // Build the pattern display with current index highlighted.
    let mut spans = Vec::new();
    for (i, &pitch) in state.pattern.iter().enumerate() {
        let note_name = midi_note_name(pitch);
        let style = if i == state.current_index {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        spans.push(Span::styled(format!("{}", pitch), style));
        spans.push(Span::raw(format!("({}) ", note_name)));
    }

    let pattern_line = Line::from(spans);

    let last_info = match state.last_pitch {
        Some(p) => format!("Last played: {} ({})", p, midi_note_name(p)),
        None => "Last played: (none)".to_string(),
    };

    let pattern_widget = Paragraph::new(vec![
        pattern_line,
        Line::from(""),
        Line::from(last_info),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Pattern "));

    frame.render_widget(pattern_widget, area);
}

/// Draw the params display.
fn draw_params(frame: &mut Frame, state: &AppState, area: Rect) {
    let params = vec![
        Line::from(vec![
            Span::styled("Trigger: ", Style::default().fg(Color::Green)),
            Span::raw(&state.trigger_address),
        ]),
        Line::from(vec![
            Span::styled("Output:  ", Style::default().fg(Color::Green)),
            Span::raw(&state.output_address),
        ]),
        Line::from(vec![
            Span::styled("Channel: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", state.channel)),
            Span::raw("  Velocity: "),
            Span::raw(format!("{}", state.velocity)),
            Span::raw("  Duration: "),
            Span::raw(format!("{:.2}s", state.duration)),
        ]),
    ];

    let params_widget = Paragraph::new(params)
        .block(Block::default().borders(Borders::ALL).title(" Params "));

    frame.render_widget(params_widget, area);
}

/// Draw the controls help.
fn draw_controls(frame: &mut Frame, area: Rect) {
    let controls = vec![
        Line::from(vec![
            Span::styled("[←/→]", Style::default().fg(Color::Yellow)),
            Span::raw(" Add/Remove pitch  "),
            Span::styled("[↑/↓]", Style::default().fg(Color::Yellow)),
            Span::raw(" Adjust current pitch"),
        ]),
        Line::from(vec![
            Span::styled("[C/c]", Style::default().fg(Color::Yellow)),
            Span::raw(" Channel ±1  "),
            Span::styled("[V/v]", Style::default().fg(Color::Yellow)),
            Span::raw(" Velocity ±10  "),
            Span::styled("[D/d]", Style::default().fg(Color::Yellow)),
            Span::raw(" Duration ±0.05"),
        ]),
        Line::from(vec![
            Span::styled("[R]", Style::default().fg(Color::Yellow)),
            Span::raw(" Edit trigger address  "),
            Span::styled("[O]", Style::default().fg(Color::Yellow)),
            Span::raw(" Edit output address  "),
            Span::styled("[Q]", Style::default().fg(Color::Yellow)),
            Span::raw(" Quit"),
        ]),
    ];

    let controls_widget = Paragraph::new(controls)
        .block(Block::default().borders(Borders::ALL).title(" Controls "));

    frame.render_widget(controls_widget, area);
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// Handle keyboard input.
async fn handle_input(state: &mut AppState, key: event::KeyEvent) {
    match key.code {
        // Quit
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            state.should_quit = true;
        }

        // Add/remove pitches: left/right arrows
        KeyCode::Left => {
            if !state.pattern.is_empty() {
                // Remove the pitch before the current index.
                let remove_idx = if state.current_index > 0 {
                    state.current_index - 1
                } else {
                    state.pattern.len() - 1
                };
                state.pattern.remove(remove_idx);
                if state.current_index >= state.pattern.len() && !state.pattern.is_empty() {
                    state.current_index = 0;
                }
                state.publish_params().await;
            }
        }
        KeyCode::Right => {
            // Add a pitch at the current position (copy of current + 1 semitone, or 60 if empty).
            let new_pitch = if state.pattern.is_empty() {
                60
            } else {
                state.pattern[state.current_index] + 1
            };
            state.pattern.insert(state.current_index, new_pitch);
            state.publish_params().await;
        }

        // Adjust current pitch: up/down arrows
        KeyCode::Up => {
            if !state.pattern.is_empty() {
                state.pattern[state.current_index] =
                    (state.pattern[state.current_index] + 1).min(127);
                state.publish_params().await;
            }
        }
        KeyCode::Down => {
            if !state.pattern.is_empty() {
                state.pattern[state.current_index] =
                    (state.pattern[state.current_index] - 1).max(0);
                state.publish_params().await;
            }
        }

        // Channel: C/c
        KeyCode::Char('C') => {
            state.channel = (state.channel + 1).min(15);
            state.publish_params().await;
        }
        KeyCode::Char('c') => {
            state.channel = (state.channel - 1).max(0);
            state.publish_params().await;
        }

        // Velocity: V/v
        KeyCode::Char('V') => {
            state.velocity = (state.velocity + 10).min(127);
            state.publish_params().await;
        }
        KeyCode::Char('v') => {
            state.velocity = (state.velocity - 10).max(0);
            state.publish_params().await;
        }

        // Duration: D/d
        KeyCode::Char('D') => {
            state.duration = (state.duration + 0.05).min(5.0);
            state.publish_params().await;
        }
        KeyCode::Char('d') => {
            state.duration = (state.duration - 0.05).max(0.01);
            state.publish_params().await;
        }

        _ => {}
    }
}

// ---------------------------------------------------------------------------
// TUI loop
// ---------------------------------------------------------------------------

/// Run the TUI.
pub async fn run_tui(state: Arc<Mutex<AppState>>) -> Result<()> {
    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        // Draw.
        {
            let s = state.lock().await;
            terminal.draw(|frame| draw(frame, &s))?;
        }

        // Handle input (non-blocking, 50ms timeout for responsive UI).
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let mut s = state.lock().await;
                    handle_input(&mut s, key).await;
                    if s.should_quit {
                        break;
                    }
                }
            }
        }
    }

    // Restore terminal.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}
