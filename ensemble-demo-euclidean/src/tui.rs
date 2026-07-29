//! TUI — terminal user interface for the Euclidean rhythm generator.
//!
//! Provides real-time visualisation of the pattern and keyboard controls
//! for BPM, steps, hits, rotation, and output address.

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
            Constraint::Length(5), // Params
            Constraint::Min(3),    // Controls
        ])
        .split(frame.area());

    draw_header(frame, state, chunks[0]);
    draw_pattern(frame, state, chunks[1]);
    draw_params(frame, state, chunks[2]);
    draw_controls(frame, chunks[3]);
}

/// Draw the header bar.
fn draw_header(frame: &mut Frame, state: &AppState, area: Rect) {
    let status = if state.paused { "PAUSED" } else { "RUNNING" };
    let header = Paragraph::new(format!(
        " Euclidean Generator | Voice #{} | {} | BPM: {:.0} | Steps: {} | Hits: {}",
        state.hub.voice_id,
        status,
        state.scheduler.bpm,
        state.scheduler.steps,
        state.scheduler.hits
    ))
    .style(Style::default().fg(Color::Cyan))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Ensemble — Euclidean Generator "),
    );
    frame.render_widget(header, area);
}

/// Draw the pattern visualization.
fn draw_pattern(frame: &mut Frame, state: &AppState, area: Rect) {
    let pattern = state.pattern();
    let current_step = state.scheduler.current_step;

    // Build the pattern display with current step highlighted.
    let mut spans = Vec::new();
    for (i, &hit) in pattern.iter().enumerate() {
        let symbol = if hit { "X" } else { "." };
        let style = if i == current_step {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if hit {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(symbol, style));
        spans.push(Span::raw(" "));
    }

    let pattern_line = Line::from(spans);
    let step_info = format!(
        "Step: {}/{} | Rotation: {}",
        current_step + 1,
        state.scheduler.steps,
        state.scheduler.rotation
    );

    let pattern_widget = Paragraph::new(vec![pattern_line, Line::from(""), Line::from(step_info)])
        .block(Block::default().borders(Borders::ALL).title(" Pattern "));

    frame.render_widget(pattern_widget, area);
}

/// Draw the params display.
fn draw_params(frame: &mut Frame, state: &AppState, area: Rect) {
    let params = vec![
        Line::from(vec![
            Span::styled("BPM: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:.0}", state.scheduler.bpm)),
        ]),
        Line::from(vec![
            Span::styled("Output: ", Style::default().fg(Color::Green)),
            Span::raw(&state.scheduler.output_address),
        ]),
    ];

    let params_widget =
        Paragraph::new(params).block(Block::default().borders(Borders::ALL).title(" Params "));

    frame.render_widget(params_widget, area);
}

/// Draw the controls help.
fn draw_controls(frame: &mut Frame, area: Rect) {
    let controls = vec![
        Line::from(vec![
            Span::styled("[←/→]", Style::default().fg(Color::Yellow)),
            Span::raw(" Steps  "),
            Span::styled("[↑/↓]", Style::default().fg(Color::Yellow)),
            Span::raw(" Hits  "),
            Span::styled("[Shift+←/→]", Style::default().fg(Color::Yellow)),
            Span::raw(" Rotation"),
        ]),
        Line::from(vec![
            Span::styled("[B/b]", Style::default().fg(Color::Yellow)),
            Span::raw(" BPM ±1  "),
            Span::styled("[Shift+B/b]", Style::default().fg(Color::Yellow)),
            Span::raw(" BPM ±10  "),
            Span::styled("[O]", Style::default().fg(Color::Yellow)),
            Span::raw(" Edit output"),
        ]),
        Line::from(vec![
            Span::styled("[Space]", Style::default().fg(Color::Yellow)),
            Span::raw(" Pause/Resume  "),
            Span::styled("[Q]", Style::default().fg(Color::Yellow)),
            Span::raw(" Quit"),
        ]),
    ];

    let controls_widget =
        Paragraph::new(controls).block(Block::default().borders(Borders::ALL).title(" Controls "));

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

        // Steps: left/right arrows (without Shift)
        KeyCode::Left if !key.modifiers.contains(event::KeyModifiers::SHIFT) => {
            if state.scheduler.steps > 1 {
                // set_steps also clamps hits so euclidean() can never panic.
                state.scheduler.set_steps(state.scheduler.steps - 1);
                state.publish_params().await;
            }
        }
        KeyCode::Right if !key.modifiers.contains(event::KeyModifiers::SHIFT) => {
            if state.scheduler.steps < 64 {
                state.scheduler.set_steps(state.scheduler.steps + 1);
                state.publish_params().await;
            }
        }

        // Hits: up/down arrows
        KeyCode::Up => {
            if state.scheduler.hits < state.scheduler.steps {
                state.scheduler.hits += 1;
                state.publish_params().await;
            }
        }
        KeyCode::Down => {
            if state.scheduler.hits > 0 {
                state.scheduler.hits -= 1;
                state.publish_params().await;
            }
        }

        // Rotation: Shift+left/right
        KeyCode::Left if key.modifiers.contains(event::KeyModifiers::SHIFT) => {
            if state.scheduler.rotation > 0 {
                state.scheduler.rotation -= 1;
                state.publish_params().await;
            }
        }
        KeyCode::Right if key.modifiers.contains(event::KeyModifiers::SHIFT) => {
            state.scheduler.rotation += 1;
            state.publish_params().await;
        }

        // BPM: case selects direction (B up, b down), Shift selects a
        // ±10 step instead of ±1. Matching both cases in one arm keeps the
        // Shift branch reachable regardless of how the terminal reports
        // shifted letters.
        KeyCode::Char(c @ ('b' | 'B')) => {
            let step = if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                10.0
            } else {
                1.0
            };
            if c.is_uppercase() {
                state.scheduler.bpm = (state.scheduler.bpm + step).min(300.0);
            } else {
                state.scheduler.bpm = (state.scheduler.bpm - step).max(20.0);
            }
            state.publish_params().await;
        }

        // Pause/Resume: Space (the scheduler task stays alive while paused,
        // so resuming is always possible).
        KeyCode::Char(' ') => {
            state.paused = !state.paused;
        }

        _ => {}
    }
}

// ---------------------------------------------------------------------------
// TUI loop
// ---------------------------------------------------------------------------

/// RAII guard that restores the terminal on drop.
///
/// Ensures raw mode is disabled and the alternate screen is left even when
/// the TUI exits via an error or a panic, so the user's terminal is never
/// stranded.
struct TerminalGuard;

impl TerminalGuard {
    /// Enter raw mode and switch to the alternate screen.
    fn new() -> Result<Self> {
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
pub async fn run_tui(state: Arc<Mutex<AppState>>) -> Result<()> {
    // Set up the terminal; the guard restores it on every exit path.
    let _guard = TerminalGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
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

    Ok(())
}
