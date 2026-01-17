use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use std::time::Duration;

/// Input events for the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// Move selection up
    Up,
    /// Move selection down
    Down,
    /// Go to top of list
    Top,
    /// Go to bottom of list
    Bottom,
    /// Scroll image panel left (previous image)
    ImageLeft,
    /// Scroll image panel right (next image)
    ImageRight,
    /// Refresh timeline
    Refresh,
    /// Open selected tweet in browser
    Open,
    /// Toggle image display
    ToggleImages,
    /// Cycle summarizer provider
    CycleSummarizer,
    /// Summarize selected tweet or link
    Summarize,
    /// Speak the last summary
    SpeakSummary,
    /// Page up the summary panel
    SummaryPageUp,
    /// Page down the summary panel
    SummaryPageDown,
    /// Mouse wheel scroll (with coordinates)
    MouseScroll {
        direction: ScrollDirection,
        column: u16,
        row: u16,
    },
    /// Toggle help overlay
    ToggleHelp,
    /// Quit the application
    Quit,
    /// No event (tick)
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
}

/// Poll for input events with a timeout
pub fn poll_event(timeout: Duration) -> Result<AppEvent> {
    if event::poll(timeout)? {
        match event::read()? {
            Event::Key(key) => return Ok(handle_key(key)),
            Event::Mouse(mouse) => return Ok(handle_mouse(mouse)),
            _ => {}
        }
    }
    Ok(AppEvent::Tick)
}

/// Map key events to app events
fn handle_key(key: KeyEvent) -> AppEvent {
    match key.code {
        // Quit
        KeyCode::Char('q') => AppEvent::Quit,
        KeyCode::Esc => AppEvent::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => AppEvent::Quit,

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => AppEvent::Down,
        KeyCode::Char('k') | KeyCode::Up => AppEvent::Up,
        KeyCode::Char('g') => AppEvent::Top,
        KeyCode::Char('G') => AppEvent::Bottom,

        // Image panel navigation
        KeyCode::Char('h') | KeyCode::Left => AppEvent::ImageLeft,
        KeyCode::Char('l') | KeyCode::Right => AppEvent::ImageRight,

        // Actions
        KeyCode::Char('r') => AppEvent::Refresh,
        KeyCode::Char('o') | KeyCode::Enter => AppEvent::Open,
        KeyCode::Char('i') => AppEvent::ToggleImages,
        KeyCode::Char('p') => AppEvent::CycleSummarizer,
        KeyCode::Char('s') => AppEvent::Summarize,
        KeyCode::Char('v') => AppEvent::SpeakSummary,
        KeyCode::PageUp => AppEvent::SummaryPageUp,
        KeyCode::PageDown => AppEvent::SummaryPageDown,
        KeyCode::Char('?') => AppEvent::ToggleHelp,

        _ => AppEvent::Tick,
    }
}

fn handle_mouse(mouse: MouseEvent) -> AppEvent {
    match mouse.kind {
        MouseEventKind::ScrollUp => AppEvent::MouseScroll {
            direction: ScrollDirection::Up,
            column: mouse.column,
            row: mouse.row,
        },
        MouseEventKind::ScrollDown => AppEvent::MouseScroll {
            direction: ScrollDirection::Down,
            column: mouse.column,
            row: mouse.row,
        },
        _ => AppEvent::Tick,
    }
}
