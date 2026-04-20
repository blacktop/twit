use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::Duration;

/// OSC 9;4 progress bar control (Ghostty/ConEmu)
pub(crate) mod progress {
    use std::io::{self, Write};

    /// Show indeterminate progress (pulsing)
    pub fn start_indeterminate() {
        let _ = io::stdout().write_all(b"\x1b]9;4;3\x1b\\");
        let _ = io::stdout().flush();
    }

    /// Show error state
    pub fn set_error() {
        let _ = io::stdout().write_all(b"\x1b]9;4;2\x1b\\");
        let _ = io::stdout().flush();
    }

    /// Clear/hide progress bar
    pub fn clear() {
        let _ = io::stdout().write_all(b"\x1b]9;4;0\x1b\\");
        let _ = io::stdout().flush();
    }
}

use crate::config::Config;
use crate::tui::event::{AppEvent, poll_event};
use crate::tui::shell::Shell;
use crate::tui::ui;
use crate::tui::view::timeline::TimelineView;

pub struct App {
    shell: Shell,
    timeline: TimelineView,
    show_help: bool,
    should_quit: bool,
}

impl App {
    pub async fn new(config: Config) -> Result<Self> {
        let (shell, bootstrap) = Shell::new(config).await?;
        let timeline = TimelineView::new(bootstrap);
        Ok(Self {
            shell,
            timeline,
            show_help: false,
            should_quit: false,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        self.timeline.refresh(&mut self.shell).await;

        let tick_rate = Duration::from_millis(100);
        let mut images_loading = false;
        while !self.should_quit {
            self.timeline
                .drain_summary_stream(&mut self.shell)
                .await;

            let more_images =
                self.timeline.load_one_image(&mut self.shell).await;
            if more_images && !images_loading {
                progress::start_indeterminate();
                images_loading = true;
            } else if !more_images && images_loading {
                progress::clear();
                images_loading = false;
            }

            terminal.draw(|f| {
                ui::render(
                    f,
                    &mut self.timeline,
                    &mut self.shell,
                    self.show_help,
                );
            })?;

            match poll_event(tick_rate)? {
                AppEvent::Quit => self.should_quit = true,
                AppEvent::Up => self.timeline.move_up(),
                AppEvent::Down => {
                    self.timeline.move_down();
                    if self.timeline.should_auto_load_more() {
                        self.timeline
                            .load_more(&mut self.shell)
                            .await;
                    }
                }
                AppEvent::Top => self.timeline.move_to_top(),
                AppEvent::Bottom => self.timeline.move_to_bottom(),
                AppEvent::ImageLeft => self.timeline.image_scroll_left(),
                AppEvent::ImageRight => {
                    self.timeline.image_scroll_right();
                }
                AppEvent::Refresh => {
                    self.timeline.refresh(&mut self.shell).await;
                }
                AppEvent::Open => {
                    if self.timeline.is_load_more_selected() {
                        self.timeline
                            .load_more(&mut self.shell)
                            .await;
                    } else {
                        self.timeline.open_selected();
                    }
                }
                AppEvent::ToggleImages => self.shell.toggle_images(),
                AppEvent::CycleSummarizer => {
                    if let Some(err) =
                        self.shell.cycle_ai_provider()
                    {
                        self.timeline.set_error("ai_init", err);
                    }
                }
                AppEvent::Summarize => {
                    self.timeline
                        .summarize_selected(&mut self.shell)
                        .await;
                }
                AppEvent::SpeakSummary => {
                    self.timeline
                        .speak_summary(&mut self.shell)
                        .await;
                }
                AppEvent::SummaryPageUp => {
                    self.timeline.summary_scroll_page(false);
                }
                AppEvent::SummaryPageDown => {
                    self.timeline.summary_scroll_page(true);
                }
                AppEvent::ToggleHelp => {
                    self.show_help = !self.show_help;
                }
                AppEvent::MouseScroll {
                    direction,
                    column,
                    row,
                } => {
                    if self
                        .timeline
                        .summary_area_contains(column, row)
                    {
                        let delta = match direction {
                            crate::tui::event::ScrollDirection::Up => {
                                -(TimelineView::SUMMARY_SCROLL_LINES
                                    as i32)
                            }
                            crate::tui::event::ScrollDirection::Down => {
                                TimelineView::SUMMARY_SCROLL_LINES as i32
                            }
                        };
                        self.timeline.summary_scroll_by(delta);
                    }
                }
                AppEvent::Tick => {
                    if self.timeline.loading {
                        self.timeline.loading_tick =
                            self.timeline.loading_tick.wrapping_add(1);
                    }
                }
            }
        }

        progress::clear();
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }
}
