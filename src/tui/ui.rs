use chrono::{DateTime, Utc};
use nerd_font_symbols::fa;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use ratatui_image::{Resize, StatefulImage};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::config::Theme;
use crate::tui::app::App;
use crate::twitter::{MediaType, Tweet};

// Nerd Font icons for tweet stats (more integrated with TUI than emojis)
const ICON_COMMENT: &str = fa::FA_COMMENT;
const ICON_RETWEET: &str = fa::FA_RETWEET;
const ICON_HEART: &str = fa::FA_HEART;
const ICON_IMAGE: &str = fa::FA_IMAGE;
const ICON_VIDEO: &str = fa::FA_VIDEO;
const ICON_FILM: &str = fa::FA_FILM;
const ICON_QUOTE: &str = fa::FA_QUOTE_LEFT;

const DEFAULT_PLACEHOLDER_PALETTE: [Color; 8] = [
    Color::Blue,
    Color::Cyan,
    Color::Green,
    Color::Magenta,
    Color::Yellow,
    Color::LightBlue,
    Color::LightCyan,
    Color::LightGreen,
];

const VIBRANT_PLACEHOLDER_PALETTE: [Color; 8] = [
    Color::Rgb(72, 132, 255),
    Color::Rgb(33, 183, 145),
    Color::Rgb(233, 92, 92),
    Color::Rgb(217, 141, 73),
    Color::Rgb(152, 92, 233),
    Color::Rgb(92, 167, 233),
    Color::Rgb(233, 92, 171),
    Color::Rgb(92, 196, 233),
];

/// Color palette for the TUI - resolved from theme
#[derive(Clone, Copy)]
pub struct ThemeColors {
    pub selection_bg: Color,
    pub handle: Color,
    pub quote: Color,
    pub timestamp: Color,
    pub stats: Color,
    pub verified: Color,
    pub separator: Color,
}

impl ThemeColors {
    pub fn from_theme(theme: Theme) -> Self {
        match theme {
            Theme::Default => Self {
                // ANSI colors - works well across terminal themes
                selection_bg: Color::DarkGray,
                handle: Color::Gray,
                quote: Color::Gray,
                timestamp: Color::DarkGray,
                stats: Color::DarkGray,
                verified: Color::Cyan,
                separator: Color::DarkGray,
            },
            #[allow(clippy::disallowed_methods)] // RGB colors intentional for vibrant theme
            Theme::Vibrant => Self {
                selection_bg: Color::Rgb(40, 44, 52),
                handle: Color::Rgb(150, 150, 150),
                quote: Color::Rgb(130, 130, 130),
                timestamp: Color::Rgb(100, 100, 100),
                stats: Color::Rgb(90, 90, 90),
                verified: Color::Rgb(29, 161, 242), // Twitter blue
                separator: Color::Rgb(60, 60, 60),
            },
        }
    }
}

/// Height of each tweet row in lines
const TWEET_ROW_HEIGHT: u16 = 7;
/// Width of avatar column (including padding)
const AVATAR_WIDTH: u16 = 8;
/// Padding around avatar
const AVATAR_PADDING: u16 = 1;

fn truncate_to_width(line: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(line) <= max_width {
        return line.to_string();
    }

    let ellipsis = "...";
    let ellipsis_width = UnicodeWidthStr::width(ellipsis);
    if max_width <= ellipsis_width {
        return ellipsis.chars().take(max_width).collect();
    }

    let target_width = max_width - ellipsis_width;
    let mut result = String::new();
    let mut current_width = 0;

    for grapheme in UnicodeSegmentation::graphemes(line, true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if current_width + grapheme_width > target_width {
            break;
        }
        result.push_str(grapheme);
        current_width += grapheme_width;
    }

    result.push_str(ellipsis);
    result
}

/// Main render function
pub fn render(frame: &mut Frame, app: &mut App) {
    let theme = ThemeColors::from_theme(app.config.theme);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(0),    // Main content
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    render_header(frame, app, chunks[0], &theme);
    render_main_content(frame, app, chunks[1], &theme);
    render_status_bar(frame, app, chunks[2], &theme);
    if app.show_help {
        render_help_popup(frame, frame.area(), &theme);
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(10);
    let height = height.min(area.height.saturating_sub(4)).max(6);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn render_help_popup(frame: &mut Frame, area: Rect, theme: &ThemeColors) {
    let popup_area = centered_rect(area, 74, 22);
    frame.render_widget(Clear, popup_area);

    let block = Block::default().borders(Borders::ALL).title("Help");
    let inner = block.inner(popup_area);
    let inner = Rect {
        x: inner.x + 1,
        y: inner.y + 1,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(2),
    };
    frame.render_widget(block, popup_area);

    let key_style = Style::default().cyan();
    let desc_style = Style::default().fg(theme.stats);
    let section_style = Style::default().bold();

    let lines = vec![
        Line::from(Span::styled("Navigation", section_style)),
        Line::from(vec![
            Span::styled("j/k, Up/Down", key_style),
            Span::styled(" move selection", desc_style),
        ]),
        Line::from(vec![
            Span::styled("g / G", key_style),
            Span::styled(" top / bottom", desc_style),
        ]),
        Line::from(vec![
            Span::styled("h/l, Left/Right", key_style),
            Span::styled(" previous/next image", desc_style),
        ]),
        Line::from(""),
        Line::from(Span::styled("Actions", section_style)),
        Line::from(vec![
            Span::styled("r", key_style),
            Span::styled(" refresh timeline", desc_style),
        ]),
        Line::from(vec![
            Span::styled("o / Enter", key_style),
            Span::styled(" open selected", desc_style),
        ]),
        Line::from(vec![
            Span::styled("i", key_style),
            Span::styled(" toggle images", desc_style),
        ]),
        Line::from(vec![
            Span::styled("p", key_style),
            Span::styled(" cycle AI provider", desc_style),
        ]),
        Line::from(vec![
            Span::styled("s", key_style),
            Span::styled(" summarize selection", desc_style),
        ]),
        Line::from(vec![
            Span::styled("v", key_style),
            Span::styled(" speak summary", desc_style),
        ]),
        Line::from(vec![
            Span::styled("PgUp/PgDn", key_style),
            Span::styled(" scroll summary panel (when visible)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("?", key_style),
            Span::styled(" toggle help", desc_style),
        ]),
        Line::from(vec![
            Span::styled("q / Esc / Ctrl+C", key_style),
            Span::styled(" quit", desc_style),
        ]),
    ];

    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, inner);
}

/// Render the header with title and last updated time on the right
fn render_header(frame: &mut Frame, app: &App, area: Rect, theme: &ThemeColors) {
    let block = Block::default().borders(Borders::ALL).dim();
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Add horizontal padding
    let padded = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    };

    // Left side: title with Twitter bird icon
    let title_spans = if app.loading {
        vec![
            Span::styled(format!("{} ", fa::FA_TWITTER), Style::default().cyan()),
            Span::styled("twit - Loading...", Style::default().cyan().bold()),
        ]
    } else {
        vec![
            Span::styled(format!("{} ", fa::FA_TWITTER), Style::default().cyan()),
            Span::styled("twit - Following", Style::default().cyan().bold()),
        ]
    };

    let title_widget = Paragraph::new(Line::from(title_spans));
    frame.render_widget(title_widget, padded);

    // Right side: last updated (if available and not loading)
    if !app.loading
        && let Some(last_refresh) = app.last_refresh
    {
        let updated_text = format_last_updated(&last_refresh);
        let updated_widget = Paragraph::new(Span::styled(
            updated_text,
            Style::default().fg(theme.timestamp),
        ))
        .alignment(ratatui::layout::Alignment::Right);
        frame.render_widget(updated_widget, padded);
    }
}

/// Determine if we should use portrait layout (side panel below instead of right)
fn is_portrait_layout(area: Rect) -> bool {
    // Portrait when rows > cols * 0.5 (terminal visually taller than wide).
    // Terminal cells are ~2x taller than wide, so 146×78 terminal is visually portrait.
    //
    // Examples:
    // - 80x24 standard: 24 > 40 = false -> landscape
    // - 146x78 left-split: 78 > 73 = true -> portrait
    // - 200x50 wide: 50 > 100 = false -> landscape
    let width = area.width as f32;
    let height = area.height as f32;
    if width == 0.0 {
        return false;
    }
    height > width * 0.5
}

/// Render main content - timeline and optional image/summary panel
/// Layout adapts based on terminal aspect ratio:
/// - Portrait (tall): side panel below timeline
/// - Landscape (wide): side panel right of timeline
fn render_main_content(frame: &mut Frame, app: &mut App, area: Rect, theme: &ThemeColors) {
    let show_summary_panel =
        app.summary_loading || app.summary.is_some() || app.summary_error.is_some();
    let selected_has_image = app.images_enabled
        && app
            .tweets
            .get(app.selected)
            .is_some_and(|t| t.media.iter().any(|m| m.media_type == MediaType::Photo));

    #[derive(Clone, Copy)]
    enum SidePanel {
        Summary,
        Image,
        None,
    }

    let side_panel = if show_summary_panel {
        SidePanel::Summary
    } else if selected_has_image && app.image_manager.is_some() {
        SidePanel::Image
    } else {
        SidePanel::None
    };

    if matches!(side_panel, SidePanel::None) {
        app.set_summary_area(None);
        render_timeline(frame, app, area, theme);
        return;
    }

    let portrait = is_portrait_layout(area);
    let direction = if portrait {
        Direction::Vertical
    } else {
        Direction::Horizontal
    };
    let split = match (side_panel, portrait) {
        (SidePanel::Summary, _) => [Constraint::Percentage(60), Constraint::Percentage(40)],
        (SidePanel::Image, true) => [Constraint::Percentage(55), Constraint::Percentage(45)],
        (SidePanel::Image, false) => [Constraint::Percentage(60), Constraint::Percentage(40)],
        (SidePanel::None, _) => unreachable!(),
    };

    let chunks = Layout::default()
        .direction(direction)
        .constraints(split)
        .split(area);

    render_timeline(frame, app, chunks[0], theme);

    match side_panel {
        SidePanel::Summary => {
            app.set_summary_area(Some(chunks[1]));
            render_summary_panel(frame, app, chunks[1], theme);
        }
        SidePanel::Image => {
            app.set_summary_area(None);
            render_image_panel(frame, app, chunks[1]);
        }
        SidePanel::None => unreachable!(),
    }
}

fn render_summary_panel(frame: &mut Frame, app: &mut App, area: Rect, theme: &ThemeColors) {
    let mut lines: Vec<Line> = Vec::new();

    if app.summary_loading {
        lines.push(Line::from(Span::styled(
            "Summarizing...",
            Style::default().dim(),
        )));
    } else if let Some(error) = &app.summary_error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().red(),
        )));
    } else if let Some(summary) = &app.summary {
        lines.push(Line::from(Span::styled(
            format!("{} · {}", summary.provider, summary.model),
            Style::default().fg(theme.stats),
        )));
        if let Some(source) = &summary.source_url {
            lines.push(Line::from(Span::styled(
                source.clone(),
                Style::default().fg(theme.stats).underlined(),
            )));
        }
        lines.push(Line::from(""));
        for line in summary.text.lines() {
            lines.push(Line::from(line.to_string()));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Press 's' to summarize the selected tweet, link, or image.",
            Style::default().dim(),
        )));
    }

    let block = Block::default().title(" Summary ").borders(Borders::ALL);
    let inner = block.inner(area);
    let mut panel = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: true });

    let content_lines = if inner.width == 0 {
        0
    } else {
        panel.line_count(inner.width)
    };
    app.set_summary_scroll_bounds(content_lines, inner.height);
    let scroll_offset = u16::try_from(app.summary_scroll).unwrap_or(u16::MAX);
    panel = panel.scroll((scroll_offset, 0));

    frame.render_widget(panel, area);

    if content_lines > inner.height as usize {
        let mut state = ScrollbarState::new(content_lines)
            .position(app.summary_scroll)
            .viewport_content_length(inner.height as usize);
        let scrollbar = Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight);
        frame.render_stateful_widget(scrollbar, inner, &mut state);
    }
}

/// Render the tweet timeline with inline avatars
fn render_timeline(frame: &mut Frame, app: &mut App, area: Rect, theme: &ThemeColors) {
    if let Some(error) = &app.error {
        let error_text = Paragraph::new(format!("Error: {}", error))
            .red()
            .block(
                Block::default()
                    .title(" Error ")
                    .borders(Borders::ALL)
                    .red(),
            )
            .wrap(Wrap { trim: true });
        frame.render_widget(error_text, area);
        return;
    }

    let timeline_len = app.timeline_len();
    if timeline_len == 0 {
        let empty = Paragraph::new("No tweets to display. Press 'r' to refresh.")
            .dim()
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(empty, area);
        return;
    }

    let mut title = format!(" {} tweets ", app.tweets.len());
    if app.has_more() {
        title.push_str("+ more ");
    }
    if app.images_enabled {
        title.push_str("[i: toggle images] ");
    }

    let block = Block::default().title(title).borders(Borders::ALL).dim();

    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    // Calculate visible tweet range based on selection and available height
    let visible_rows = (inner_area.height / TWEET_ROW_HEIGHT) as usize;
    let half_visible = visible_rows / 2;

    // Center selection in view when possible
    let scroll_offset = if app.selected < half_visible {
        0
    } else if app.selected + half_visible >= timeline_len {
        timeline_len.saturating_sub(visible_rows)
    } else {
        app.selected.saturating_sub(half_visible)
    };

    // Collect visible tweet indices to avoid borrow issues
    let visible_range: Vec<usize> = (scroll_offset..)
        .take(visible_rows + 1)
        .take_while(|&i| i < timeline_len)
        .collect();

    // Render visible tweets
    let mut y = inner_area.y;
    for i in visible_range {
        if y + TWEET_ROW_HEIGHT > inner_area.y + inner_area.height {
            break;
        }

        let tweet_area = Rect {
            x: inner_area.x,
            y,
            width: inner_area.width,
            height: TWEET_ROW_HEIGHT,
        };

        let is_selected = i == app.selected;
        if i < app.tweets.len() {
            render_tweet_row(frame, app, i, tweet_area, is_selected, theme);
        } else {
            render_load_more_row(frame, app, tweet_area, is_selected, theme);
        }

        y += TWEET_ROW_HEIGHT;
    }

    // Render scrollbar
    if timeline_len > visible_rows {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = ScrollbarState::new(timeline_len).position(app.selected);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

/// Render a single tweet row with avatar
fn render_tweet_row(
    frame: &mut Frame,
    app: &mut App,
    tweet_idx: usize,
    area: Rect,
    is_selected: bool,
    theme: &ThemeColors,
) {
    let Some(tweet) = app.tweets.get(tweet_idx) else {
        return;
    };

    // Clone what we need to avoid borrow issues
    let user_name = tweet.user.name.clone();
    let avatar_url = tweet.user.avatar_url_bigger();
    let images_enabled = app.images_enabled;

    // Format content before borrowing app mutably
    let max_width = app.config.tweet_max_width;
    let content = format_tweet_compact(tweet, is_selected && images_enabled, max_width, theme);

    // Split into avatar column and content
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(AVATAR_WIDTH), // Avatar with padding
            Constraint::Min(0),               // Content
        ])
        .split(area);

    // Render avatar with padding on all sides
    render_avatar(frame, app, &avatar_url, &user_name, chunks[0]);

    // Render tweet content with word wrapping
    // Use subtle background highlight for selection (not jarring white reversed)
    let paragraph = if is_selected {
        Paragraph::new(content)
            .wrap(Wrap { trim: true })
            .bg(theme.selection_bg)
    } else {
        Paragraph::new(content).wrap(Wrap { trim: true })
    };

    frame.render_widget(paragraph, chunks[1]);
}

fn render_load_more_row(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    is_selected: bool,
    theme: &ThemeColors,
) {
    let label = if app.loading_more {
        "Loading more..."
    } else {
        "[ Load more... ]"
    };

    let style = if is_selected {
        Style::default().fg(theme.stats).bg(theme.selection_bg)
    } else {
        Style::default().fg(theme.stats).dim()
    };

    let paragraph = Paragraph::new(label)
        .style(style)
        .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(paragraph, area);
}

/// Render user avatar with padding on all sides
fn render_avatar(frame: &mut Frame, app: &mut App, avatar_url: &str, user_name: &str, area: Rect) {
    let padded_area = area.inner(ratatui::layout::Margin {
        horizontal: AVATAR_PADDING,
        vertical: AVATAR_PADDING,
    });

    let Some(ref mut image_manager) = app.image_manager else {
        render_avatar_placeholder(frame, padded_area, user_name, avatar_url, app.config.theme);
        return;
    };

    if !app.images_enabled || avatar_url.is_empty() {
        render_avatar_placeholder(frame, padded_area, user_name, avatar_url, app.config.theme);
        return;
    }

    if let Some(protocol) = image_manager.get_protocol(avatar_url) {
        let image_widget = StatefulImage::new().resize(Resize::Fit(None));
        frame.render_stateful_widget(image_widget, padded_area, protocol);
    } else {
        render_avatar_placeholder(frame, padded_area, user_name, avatar_url, app.config.theme);
    }
}

/// Get initials from a name (for avatar placeholder)
fn get_initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn placeholder_color(key: &str, theme: Theme) -> Color {
    let palette = match theme {
        Theme::Default => &DEFAULT_PLACEHOLDER_PALETTE,
        Theme::Vibrant => &VIBRANT_PLACEHOLDER_PALETTE,
    };
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % palette.len();
    palette[idx]
}

fn placeholder_text_color(bg: Color) -> Color {
    match bg {
        Color::Rgb(r, g, b) => {
            let luminance =
                (0.299 * f64::from(r)) + (0.587 * f64::from(g)) + (0.114 * f64::from(b));
            if luminance > 150.0 {
                Color::Black
            } else {
                Color::White
            }
        }
        Color::White
        | Color::Gray
        | Color::LightBlue
        | Color::LightCyan
        | Color::LightGreen
        | Color::LightYellow
        | Color::LightMagenta => Color::Black,
        _ => Color::White,
    }
}

fn render_avatar_placeholder(
    frame: &mut Frame,
    area: Rect,
    user_name: &str,
    key: &str,
    theme: Theme,
) {
    let initials = get_initials(user_name);
    let bg = placeholder_color(key, theme);
    let fg = placeholder_text_color(bg);
    let block = Block::default().style(Style::default().bg(bg));
    frame.render_widget(block, area);

    let text = Paragraph::new(initials)
        .style(Style::default().fg(fg).bg(bg).bold())
        .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(text, area);
}

fn render_image_placeholder(frame: &mut Frame, area: Rect, key: &str, theme: Theme) {
    let bg = placeholder_color(key, theme);
    let fg = placeholder_text_color(bg);

    let block = Block::default().style(Style::default().bg(bg));
    frame.render_widget(block, area);

    let pattern = "░▒";
    let line = pattern
        .repeat(area.width.saturating_div(pattern.len() as u16) as usize + 1)
        .chars()
        .take(area.width as usize)
        .collect::<String>();
    let mut lines = Vec::new();
    for _ in 0..area.height {
        lines.push(Line::from(Span::styled(
            line.clone(),
            Style::default().fg(fg).bg(bg).dim(),
        )));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), area);

    let text_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    let label = Paragraph::new(format!("{} Loading image...", ICON_IMAGE))
        .style(Style::default().fg(fg).bg(bg).bold())
        .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(label, text_layout[1]);
}

/// Render the image panel for the selected tweet (supports multiple images with h/l scrolling)
fn render_image_panel(frame: &mut Frame, app: &mut App, area: Rect) {
    let image_urls: Vec<String> = app
        .tweets
        .get(app.selected)
        .map(|t| app.get_tweet_image_urls(t))
        .unwrap_or_default();

    let image_count = image_urls.len();

    let title = if image_count > 1 {
        format!(" Image {}/{} (h/l) ", app.image_scroll + 1, image_count)
    } else {
        " Image ".to_string()
    };

    let block = Block::default().title(title).borders(Borders::ALL).dim();
    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    let Some(current_url) = image_urls.get(app.image_scroll.min(image_count.saturating_sub(1)))
    else {
        return;
    };

    let Some(ref mut image_manager) = app.image_manager else {
        render_image_placeholder(frame, inner_area, current_url, app.config.theme);
        return;
    };

    if let Some(protocol) = image_manager.get_protocol(current_url) {
        let image_widget = StatefulImage::new().resize(Resize::Scale(None));
        frame.render_stateful_widget(image_widget, inner_area, protocol);
    } else {
        render_image_placeholder(frame, inner_area, current_url, app.config.theme);
    }
}

/// Format a tweet compactly for the avatar layout (fits in TWEET_ROW_HEIGHT lines)
/// Color hierarchy (dark mode best practices):
/// - Name: default (bright, prominent)
/// - Handle: muted gray (secondary info)
/// - Verified badge: Twitter blue
/// - Timestamp: darker gray
/// - Tweet text: default (readable)
/// - Stats: darkest gray (UI chrome, not content)
fn format_tweet_compact(
    tweet: &Tweet,
    is_selected_with_images: bool,
    max_width: usize,
    theme: &ThemeColors,
) -> Text<'static> {
    let mut lines = Vec::new();

    // Line 1: User info with proper color hierarchy
    let mut user_spans: Vec<Span> = vec![
        // Name: bold, default color (most prominent)
        tweet.user.name.clone().bold(),
        " ".into(),
        // Handle: muted gray (secondary)
        Span::styled(
            format!("@{}", tweet.user.screen_name),
            Style::default().fg(theme.handle),
        ),
    ];

    // Verified badge: Twitter blue (brand color)
    if tweet.user.verified || tweet.user.blue_verified {
        user_spans.push(Span::styled(" ✓", Style::default().fg(theme.verified)));
    }

    // Timestamp: even more muted
    user_spans.push(Span::styled(
        format!(" · {}", format_relative_time(&tweet.created_at)),
        Style::default().fg(theme.timestamp),
    ));

    lines.push(Line::from(user_spans));

    // Line 2: Retweet indicator or first line of text
    if let Some(ref retweeted_by) = tweet.retweeted_by {
        lines.push(Line::from(vec![format!("↻ RT @{}", retweeted_by).green()]));
    }

    // Tweet text (default color - this is the content users want to read)
    let text_lines: Vec<&str> = tweet.text.lines().collect();
    let has_retweet = tweet.retweeted_by.is_some();
    let has_quote = tweet.quoted_tweet.is_some();

    // Allocate text lines based on available space (RT and quote take lines)
    let max_text_lines = match (has_retweet, has_quote) {
        (true, true) => 1,
        (true, false) | (false, true) => 2,
        (false, false) => 3,
    };

    for line in text_lines.iter().take(max_text_lines) {
        // Truncate long lines by display width to avoid overflow and UTF-8 issues
        let display_line = truncate_to_width(line, max_width);
        // Tweet text uses default terminal foreground (readable)
        lines.push(Line::from(display_line));
    }

    // Show quoted tweet if present
    if let Some(ref quoted) = tweet.quoted_tweet {
        // Quote header: icon + user info
        let quote_header = Line::from(vec![
            Span::styled(format!("{} ", ICON_QUOTE), Style::default().fg(theme.quote)),
            Span::styled(
                quoted.user.name.clone(),
                Style::default().fg(theme.quote).bold(),
            ),
            Span::styled(
                format!(" @{}: ", quoted.user.screen_name),
                Style::default().fg(theme.handle),
            ),
        ]);
        lines.push(quote_header);

        // Quote text (truncated to fit)
        let quote_text = quoted.text.lines().next().unwrap_or("");
        let truncated_quote = truncate_to_width(quote_text, max_width.saturating_sub(2));
        lines.push(Line::from(Span::styled(
            format!("  {}", truncated_quote),
            Style::default().fg(theme.quote),
        )));
    }

    // Pad with empty lines if needed
    while lines.len() < 4 {
        lines.push(Line::from(""));
    }

    // Line 5: Media indicator + stats (UI chrome - darkest, recedes visually)
    let mut info_spans: Vec<Span> = Vec::new();

    if !tweet.media.is_empty() {
        let has_photo = tweet.media.iter().any(|m| m.media_type == MediaType::Photo);

        if has_photo && is_selected_with_images {
            info_spans.push(format!("[{}→] ", ICON_IMAGE).cyan().bold());
        } else {
            let media_icons: String = tweet
                .media
                .iter()
                .map(|m| match m.media_type {
                    MediaType::Photo => ICON_IMAGE,
                    MediaType::Video => ICON_VIDEO,
                    MediaType::Gif => ICON_FILM,
                })
                .collect::<Vec<_>>()
                .join("");
            info_spans.push(Span::styled(
                format!("[{}] ", media_icons),
                Style::default().fg(theme.stats).bold(),
            ));
        }
    }

    // Stats: darkest gray (UI elements should recede, not compete with content)
    info_spans.extend([
        Span::styled(
            format!("{} ", ICON_COMMENT),
            Style::default().fg(theme.stats).bold(),
        ),
        Span::styled(
            format_count(tweet.reply_count),
            Style::default().fg(theme.stats),
        ),
        Span::styled("  ", Style::default().fg(theme.stats)),
        Span::styled(
            format!("{} ", ICON_RETWEET),
            Style::default().fg(theme.stats).bold(),
        ),
        Span::styled(
            format_count(tweet.retweet_count),
            Style::default().fg(theme.stats),
        ),
        Span::styled("  ", Style::default().fg(theme.stats)),
        Span::styled(
            format!("{} ", ICON_HEART),
            Style::default().fg(theme.stats).bold(),
        ),
        Span::styled(
            format_count(tweet.like_count),
            Style::default().fg(theme.stats),
        ),
    ]);

    lines.push(Line::from(info_spans));

    // Line 6: Separator (subtle, not distracting)
    lines.push(Line::from(Span::styled(
        "─".repeat(70),
        Style::default().fg(theme.separator),
    )));

    Text::from(lines)
}

/// Render the status bar with styled key legend
fn render_status_bar(frame: &mut Frame, app: &App, area: Rect, theme: &ThemeColors) {
    let images_status = if app.images_enabled { "on" } else { "off" };
    let ai_provider = app.config.ai.provider.clone();
    let show_summary_panel =
        app.summary_loading || app.summary.is_some() || app.summary_error.is_some();

    let mut spans: Vec<Span> = Vec::new();

    // Left padding
    spans.push(" ".into());

    if app.loading {
        spans.push(Span::styled("Loading...", Style::default().dim()));
    } else {
        // Keys in cyan (like header), descriptions in muted gray
        spans.push(Span::styled("j/k", Style::default().cyan()));
        spans.push(Span::styled(" nav", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        spans.push(Span::styled("g/G", Style::default().cyan()));
        spans.push(Span::styled(" top/btm", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        spans.push(Span::styled("h/l", Style::default().cyan()));
        spans.push(Span::styled(" images", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        spans.push(Span::styled("r", Style::default().cyan()));
        spans.push(Span::styled(" refresh", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        spans.push(Span::styled("o", Style::default().cyan()));
        spans.push(Span::styled(" open", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        spans.push(Span::styled("i", Style::default().cyan()));
        spans.push(Span::styled(
            format!(" toggle ({})", images_status),
            Style::default().fg(theme.stats),
        ));
        spans.push("  ".into());

        spans.push(Span::styled("p", Style::default().cyan()));
        spans.push(Span::styled(
            format!(" provider ({})", ai_provider),
            Style::default().fg(theme.stats),
        ));
        spans.push("  ".into());

        spans.push(Span::styled("s", Style::default().cyan()));
        spans.push(Span::styled(" summarize", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        spans.push(Span::styled("v", Style::default().cyan()));
        spans.push(Span::styled(" speak", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        if show_summary_panel {
            spans.push(Span::styled("PgUp/PgDn", Style::default().cyan()));
            spans.push(Span::styled(" summary", Style::default().fg(theme.stats)));
            spans.push("  ".into());
        }

        spans.push(Span::styled("?", Style::default().cyan()));
        spans.push(Span::styled(" help", Style::default().fg(theme.stats)));
        spans.push("  ".into());

        spans.push(Span::styled("q", Style::default().cyan()));
        spans.push(Span::styled(" quit", Style::default().fg(theme.stats)));
    }

    let status_bar = Paragraph::new(Line::from(spans));
    frame.render_widget(status_bar, area);
}

/// Format a number for display (e.g., 1.2K, 3.4M)
fn format_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Format a timestamp as relative time (e.g., "2m", "1h", "3d")
fn format_relative_time(time: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*time);

    if duration.num_seconds() < 60 {
        format!("{}s", duration.num_seconds())
    } else if duration.num_minutes() < 60 {
        format!("{}m", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{}h", duration.num_hours())
    } else if duration.num_days() < 7 {
        format!("{}d", duration.num_days())
    } else {
        time.format("%b %d").to_string()
    }
}

/// Format "last updated" time - shows "just now" for < 1 minute, then "Xm ago"
fn format_last_updated(time: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*time);

    if duration.num_seconds() < 60 {
        "just now".to_string()
    } else if duration.num_minutes() < 60 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{}h ago", duration.num_hours())
    } else {
        format!("{}d ago", duration.num_days())
    }
}
