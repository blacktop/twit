use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::collections::HashSet;
use std::io;
use std::time::Duration;
use tokio::sync::mpsc::{self, error::TryRecvError};

/// OSC 9;4 progress bar control (Ghostty/ConEmu)
mod progress {
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

use crate::ai::{AiClient, ImageSummaryInput, SummaryInput, TtsClient, extract_urls};
use crate::config::{AiProvider, Config};
use crate::logging;
use crate::tui::event::{AppEvent, poll_event};
use crate::tui::ui;
use crate::twitter::{Tweet, TweetCache, TwitterClient};
use crate::widgets::ImageManager;
use ratatui::layout::Rect;
use reqwest::Url;

pub struct App {
    pub config: Config,
    client: TwitterClient,
    cache: TweetCache,

    pub tweets: Vec<Tweet>,
    pub selected: usize,

    pub loading: bool,
    pub loading_tick: u8,
    pub error: Option<String>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub latest_loaded_at: Option<DateTime<Utc>>,
    pub loaded_from_cache: bool,

    pub image_manager: Option<ImageManager>,
    pub images_enabled: bool,
    pub image_scroll: usize, // Scroll offset for image panel (when multiple images)

    pub ai: Option<AiClient>,
    pub tts: Option<TtsClient>,

    pub summary: Option<SummaryState>,
    pub summary_loading: bool,
    pub summary_error: Option<String>,
    summary_stream: Option<mpsc::Receiver<String>>,
    pending_auto_speak: bool,
    pub summary_scroll: usize,
    summary_area: Option<Rect>,
    summary_viewport_height: u16,
    summary_content_height: usize,
    pub next_cursor: Option<String>,
    pub loading_more: bool,
    pub show_help: bool,

    should_quit: bool,
}

#[derive(Debug, Clone)]
pub struct SummaryState {
    pub text: String,
    pub source_url: Option<String>,
    pub provider: AiProvider,
    pub model: String,
}

impl App {
    const SUMMARY_SCROLL_LINES: usize = 3;

    pub async fn new(config: Config) -> Result<Self> {
        let mut config = config;
        let client = TwitterClient::new(config.auth_token.clone(), config.ct0.clone())?;
        let cache = TweetCache::open()?;

        // Load cached tweets for instant display
        let tweets = cache.get_tweets().unwrap_or_default();
        let latest_loaded_at = cache
            .latest_tweet_time()
            .ok()
            .flatten()
            .or_else(|| tweets.iter().map(|tweet| tweet.created_at).max());
        let loaded_from_cache = !tweets.is_empty();

        // Try to initialize image manager (may fail if terminal doesn't support it)
        let (image_manager, images_enabled) = if config.show_images {
            match ImageManager::new_with_cache_limit(config.image_cache_max_mb) {
                Ok(mgr) => (Some(mgr), true),
                Err(_) => (None, false),
            }
        } else {
            (None, false)
        };

        let mut error = None;
        let ai = if config.ai.enabled {
            match AiClient::new(config.ai.clone()) {
                Ok(client) => Some(client),
                Err(err) => {
                    let message = format!("AI init failed: {}", err);
                    error = Some(message.clone());
                    logging::log_error("ai_init", &message);
                    config.ai.enabled = false;
                    None
                }
            }
        } else {
            None
        };

        let tts = if config.tts.enabled {
            Some(TtsClient::new(config.tts.clone()))
        } else {
            None
        };

        Ok(Self {
            config,
            client,
            cache,
            tweets,
            selected: 0,
            loading: false,
            loading_tick: 0,
            error,
            last_refresh: None,
            latest_loaded_at,
            loaded_from_cache,
            image_manager,
            images_enabled,
            image_scroll: 0,
            ai,
            tts,
            summary: None,
            summary_loading: false,
            summary_error: None,
            summary_stream: None,
            pending_auto_speak: false,
            summary_scroll: 0,
            summary_area: None,
            summary_viewport_height: 0,
            summary_content_height: 0,
            next_cursor: None,
            loading_more: false,
            show_help: false,
            should_quit: false,
        })
    }

    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }

    pub fn timeline_len(&self) -> usize {
        self.tweets.len() + usize::from(self.has_more())
    }

    pub fn is_load_more_selected(&self) -> bool {
        self.has_more() && self.selected == self.tweets.len()
    }

    fn set_error(&mut self, context: &str, message: impl Into<String>) {
        let message = message.into();
        self.error = Some(message.clone());
        logging::log_error(context, &message);
    }

    fn set_summary_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.summary_error = Some(message.clone());
        logging::log_error("summary", &message);
    }

    pub async fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Refresh on start to get latest tweets
        self.refresh().await;

        // Main loop
        let tick_rate = Duration::from_millis(100);
        let mut images_loading = false;
        while !self.should_quit {
            self.drain_summary_stream().await;

            // Load ONE image per tick (keeps UI responsive)
            let more_images = self.load_one_image().await;
            if more_images && !images_loading {
                // Just started loading images
                progress::start_indeterminate();
                images_loading = true;
            } else if !more_images && images_loading {
                // Finished loading all images
                progress::clear();
                images_loading = false;
            }

            // Draw - need mutable self for image rendering
            terminal.draw(|f| ui::render(f, self))?;

            // Handle events
            match poll_event(tick_rate)? {
                AppEvent::Quit => self.should_quit = true,
                AppEvent::Up => self.move_up(),
                AppEvent::Down => self.move_down(),
                AppEvent::Top => self.move_to_top(),
                AppEvent::Bottom => self.move_to_bottom(),
                AppEvent::ImageLeft => self.image_scroll_left(),
                AppEvent::ImageRight => self.image_scroll_right(),
                AppEvent::Refresh => self.refresh().await,
                AppEvent::Open => {
                    if self.is_load_more_selected() {
                        self.load_more().await;
                    } else {
                        self.open_selected();
                    }
                }
                AppEvent::ToggleImages => self.toggle_images(),
                AppEvent::CycleSummarizer => self.cycle_ai_provider(),
                AppEvent::Summarize => self.summarize_selected().await,
                AppEvent::SpeakSummary => self.speak_summary().await,
                AppEvent::SummaryPageUp => self.summary_scroll_page(false),
                AppEvent::SummaryPageDown => self.summary_scroll_page(true),
                AppEvent::ToggleHelp => self.toggle_help(),
                AppEvent::MouseScroll {
                    direction,
                    column,
                    row,
                } => {
                    if self.summary_area_contains(column, row) {
                        let delta = match direction {
                            crate::tui::event::ScrollDirection::Up => {
                                -(Self::SUMMARY_SCROLL_LINES as i32)
                            }
                            crate::tui::event::ScrollDirection::Down => {
                                Self::SUMMARY_SCROLL_LINES as i32
                            }
                        };
                        self.summary_scroll_by(delta);
                    }
                }
                AppEvent::Tick => {
                    // Animate loading spinner
                    if self.loading {
                        self.loading_tick = self.loading_tick.wrapping_add(1);
                    }
                }
            }
        }

        // Clear any lingering progress bar and restore terminal
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

    async fn refresh(&mut self) {
        self.loading = true;
        self.error = None;
        progress::start_indeterminate();

        match self
            .client
            .get_home_timeline_page(self.config.tweet_count, None)
            .await
        {
            Ok(page) => {
                let new_tweets = page.tweets;

                let merged = if self.loaded_from_cache {
                    new_tweets.clone()
                } else {
                    merge_newer(&self.tweets, &new_tweets)
                };

                if let Err(e) = self.cache.save_tweets(&new_tweets) {
                    eprintln!("Failed to cache tweets: {}", e);
                }

                // Clear old cache entries
                let ttl = Duration::from_secs(self.config.cache_ttl_mins * 60);
                let _ = self.cache.clear_old(ttl);

                self.tweets = merged;
                self.next_cursor = page.next_cursor;
                self.last_refresh = Some(Utc::now());
                self.latest_loaded_at = self.tweets.iter().map(|tweet| tweet.created_at).max();
                self.loaded_from_cache = false;
                self.clear_summary();

                // Reset selection if it's out of bounds
                if self.selected >= self.timeline_len() {
                    self.selected = 0;
                }

                // Eagerly load all avatars
                self.load_all_avatars().await;
            }
            Err(e) => {
                self.set_error("refresh", format!("{:#}", e));
                progress::set_error();
            }
        }

        progress::clear();
        self.loading = false;
    }

    async fn load_more(&mut self) {
        if self.loading_more {
            return;
        }
        let Some(cursor) = self.next_cursor.clone() else {
            return;
        };

        self.loading_more = true;
        progress::start_indeterminate();
        match self
            .client
            .get_home_timeline_page(self.config.tweet_count, Some(&cursor))
            .await
        {
            Ok(page) => {
                let mut older = page.tweets;
                let existing_ids: HashSet<String> =
                    self.tweets.iter().map(|tweet| tweet.id.clone()).collect();
                older.retain(|tweet| !existing_ids.contains(&tweet.id));

                if let Err(e) = self.cache.save_tweets(&older) {
                    eprintln!("Failed to cache tweets: {}", e);
                }

                append_unique(&mut self.tweets, &older);
                self.next_cursor = page.next_cursor;

                if self.selected >= self.timeline_len() {
                    self.selected = self.timeline_len().saturating_sub(1);
                }

                // Eagerly load avatars for new tweets
                self.load_all_avatars().await;
            }
            Err(err) => {
                self.set_error("load_more", format!("{:#}", err));
                progress::set_error();
            }
        }

        progress::clear();
        self.loading_more = false;
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.image_scroll = 0; // Reset image scroll when changing tweets
            self.clear_summary();
        }
    }

    fn move_down(&mut self) {
        let max_index = self.timeline_len().saturating_sub(1);
        if self.timeline_len() > 0 && self.selected < max_index {
            self.selected += 1;
            self.image_scroll = 0; // Reset image scroll when changing tweets
            self.clear_summary();
        }
    }

    fn move_to_top(&mut self) {
        self.selected = 0;
        self.image_scroll = 0;
        self.clear_summary();
    }

    fn move_to_bottom(&mut self) {
        if self.timeline_len() > 0 {
            self.selected = self.timeline_len().saturating_sub(1);
            self.image_scroll = 0;
            self.clear_summary();
        }
    }

    fn image_scroll_left(&mut self) {
        if self.image_scroll > 0 {
            self.image_scroll -= 1;
        }
    }

    fn image_scroll_right(&mut self) {
        // Get max images for current tweet
        if let Some(tweet) = self.tweets.get(self.selected) {
            let image_count = self.get_tweet_image_urls(tweet).len();
            if image_count > 0 && self.image_scroll < image_count - 1 {
                self.image_scroll += 1;
            }
        }
    }

    fn open_selected(&self) {
        if let Some(tweet) = self.tweets.get(self.selected) {
            let url = tweet.url();
            let _ = open::that(&url);
        }
    }

    fn toggle_images(&mut self) {
        self.images_enabled = !self.images_enabled;
        if self.images_enabled && self.image_manager.is_none() {
            self.image_manager =
                ImageManager::new_with_cache_limit(self.config.image_cache_max_mb).ok();
        }
    }

    /// Load ALL avatars eagerly (greedy loading for instant display)
    /// Shows progress bar during loading
    async fn load_all_avatars(&mut self) {
        if !self.images_enabled {
            return;
        }
        let Some(image_manager) = self.image_manager.as_mut() else {
            return;
        };

        // Collect all avatar URLs that need loading
        let urls: Vec<String> = self
            .tweets
            .iter()
            .map(|t| t.user.avatar_url_bigger())
            .filter(|url| !url.is_empty() && !image_manager.is_in_memory(url))
            .collect();

        if urls.is_empty() {
            return;
        }

        // Show progress while loading avatars
        progress::start_indeterminate();

        // Load all avatars
        for url in urls {
            let _ = image_manager.load_image(&url).await;
        }

        progress::clear();
    }

    /// Get pending image URLs based on config prefetch strategy.
    /// Selected tweet's images are prioritized first.
    fn pending_image_urls(&self) -> Vec<String> {
        let Some(image_manager) = self.image_manager.as_ref().filter(|_| self.images_enabled)
        else {
            return Vec::new();
        };

        use crate::config::MediaPrefetch;

        let mut urls = Vec::new();

        // Collect URLs from a tweet, filtering already-loaded ones
        let mut collect_from_tweet = |tweet: &Tweet| {
            for media in &tweet.media {
                if let Some(url) = media.small_url()
                    && !image_manager.is_in_memory(&url)
                    && !urls.contains(&url)
                {
                    urls.push(url);
                }
            }
        };

        // Priority 1: Selected tweet's images
        if let Some(tweet) = self.tweets.get(self.selected) {
            collect_from_tweet(tweet);
        }

        // Priority 2: Remaining images based on prefetch strategy
        let (start, end) = match self.config.media_prefetch {
            MediaPrefetch::Nearby => (
                self.selected.saturating_sub(5),
                (self.selected + 6).min(self.tweets.len()),
            ),
            MediaPrefetch::Visible => (
                self.selected.saturating_sub(5),
                (self.selected + 10).min(self.tweets.len()),
            ),
            MediaPrefetch::All => (0, self.tweets.len()),
        };

        for (i, tweet) in self.tweets.iter().enumerate().skip(start).take(end - start) {
            if i != self.selected {
                collect_from_tweet(tweet);
            }
        }

        urls
    }

    /// Load ONE pending media image (called each tick for responsive UI).
    /// Returns true if there are more images to load.
    async fn load_one_image(&mut self) -> bool {
        let pending = self.pending_image_urls();
        if let Some(url) = pending.first()
            && let Some(image_manager) = self.image_manager.as_mut()
        {
            let _ = image_manager.load_image(url).await;
        }
        pending.len() > 1
    }

    fn cycle_ai_provider(&mut self) {
        self.config.ai.provider = self.config.ai.provider.clone().next();
        if self.config.ai.enabled {
            self.rebuild_ai_client();
        }
    }

    fn rebuild_ai_client(&mut self) {
        match AiClient::new(self.config.ai.clone()) {
            Ok(client) => {
                self.ai = Some(client);
            }
            Err(err) => {
                self.set_error("ai_init", format!("AI init failed: {}", err));
                self.ai = None;
                self.config.ai.enabled = false;
            }
        }
    }

    fn clear_summary(&mut self) {
        self.summary = None;
        self.summary_loading = false;
        self.summary_error = None;
        self.summary_stream = None;
        self.pending_auto_speak = false;
        self.summary_scroll = 0;
    }

    pub fn set_summary_area(&mut self, area: Option<Rect>) {
        self.summary_area = area;
    }

    pub fn set_summary_scroll_bounds(&mut self, content_height: usize, viewport_height: u16) {
        self.summary_content_height = content_height;
        self.summary_viewport_height = viewport_height;
        let max_scroll = self.summary_max_scroll();
        if self.summary_scroll > max_scroll {
            self.summary_scroll = max_scroll;
        }
    }

    fn summary_max_scroll(&self) -> usize {
        let viewport = self.summary_viewport_height as usize;
        self.summary_content_height.saturating_sub(viewport)
    }

    fn summary_scroll_by(&mut self, delta: i32) {
        let max_scroll = self.summary_max_scroll();
        let new_scroll = self
            .summary_scroll
            .saturating_add_signed(delta as isize)
            .min(max_scroll);
        self.summary_scroll = new_scroll;
    }

    fn summary_scroll_page(&mut self, down: bool) {
        let step = self.summary_viewport_height.saturating_sub(1).max(1) as i32;
        let delta = if down { step } else { -step };
        self.summary_scroll_by(delta);
    }

    fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    fn summary_area_contains(&self, column: u16, row: u16) -> bool {
        self.summary_area
            .is_some_and(|area| area.contains(ratatui::layout::Position { x: column, y: row }))
    }

    async fn drain_summary_stream(&mut self) {
        let Some(rx) = self.summary_stream.as_mut() else {
            if self.pending_auto_speak {
                self.pending_auto_speak = false;
                self.speak_summary().await;
            }
            return;
        };

        let mut received = false;
        loop {
            match rx.try_recv() {
                Ok(chunk) => {
                    received = true;
                    if let Some(summary) = self.summary.as_mut() {
                        summary.text.push_str(&chunk);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.summary_stream = None;
                    self.summary_loading = false;
                    progress::clear();
                    if self.config.tts.enabled && self.config.tts.auto_speak_summaries {
                        self.pending_auto_speak = true;
                    }
                    break;
                }
            }
        }

        if received {
            progress::clear();
        }

        if self.pending_auto_speak && self.summary_stream.is_none() {
            self.pending_auto_speak = false;
            self.speak_summary().await;
        }
    }

    async fn summarize_selected(&mut self) {
        self.summary_loading = true;
        self.summary_error = None;
        self.summary = None;
        self.summary_stream = None;
        self.pending_auto_speak = false;
        self.summary_scroll = 0;

        if self.is_load_more_selected() {
            self.set_summary_error("Select a tweet to summarize");
            self.summary_loading = false;
            return;
        }

        if !self.config.ai.enabled {
            self.set_summary_error("AI summaries are disabled");
            self.summary_loading = false;
            return;
        }

        let (tweet_text, tweet_url, image_urls) = {
            let Some(tweet) = self.tweets.get(self.selected) else {
                self.set_summary_error("No tweet selected");
                self.summary_loading = false;
                return;
            };
            (
                build_summary_text(tweet),
                tweet.url(),
                self.get_tweet_image_urls_for_ai(tweet),
            )
        };

        let urls = extract_urls(&tweet_text);
        let summarize_links = self.config.ai.summarize_links;
        let summarize_tweets = self.config.ai.summarize_tweets;
        let summarize_images = self.config.ai.summarize_images;

        let link_candidates = filter_summary_urls_for_images(&urls, !image_urls.is_empty());
        let has_link_to_summarize = summarize_links && !link_candidates.is_empty();
        let has_images_to_summarize = summarize_images && !image_urls.is_empty();
        let has_tweet_to_summarize = summarize_tweets && !tweet_text.trim().is_empty();
        if !has_link_to_summarize && !has_images_to_summarize && !has_tweet_to_summarize {
            self.set_summary_error("No link, tweet, or image to summarize");
            self.summary_loading = false;
            return;
        }

        let will_use_images = !has_link_to_summarize && has_images_to_summarize;
        if will_use_images && !self.config.ai.provider.supports_image_summaries() {
            self.set_summary_error(format!(
                "AI provider {} does not support image summaries yet",
                self.config.ai.provider
            ));
            self.summary_loading = false;
            return;
        }

        if self.ai.is_none() {
            self.rebuild_ai_client();
        }

        let Some(ai) = self.ai.take() else {
            self.set_summary_error("AI client unavailable");
            self.summary_loading = false;
            return;
        };

        progress::start_indeterminate();

        if will_use_images {
            let result = ai
                .summarize_images(ImageSummaryInput {
                    tweet_text: Some(tweet_text),
                    image_urls,
                    source_url: Some(tweet_url.clone()),
                })
                .await;
            progress::clear();
            self.ai = Some(ai);
            match result {
                Ok(summary) => {
                    self.summary = Some(SummaryState {
                        text: summary.summary,
                        source_url: summary.source_url,
                        provider: summary.provider,
                        model: summary.model,
                    });
                    if self.config.tts.enabled && self.config.tts.auto_speak_summaries {
                        self.speak_summary().await;
                    }
                }
                Err(err) => {
                    self.set_summary_error(format_ai_error(&err, &self.config.ai.provider));
                }
            }
            self.summary_loading = false;
            return;
        }

        let result = if has_link_to_summarize {
            // Try URL first, fall back to tweet text if URL fetch fails
            match ai.summarize_url_streaming(&link_candidates[0]).await {
                Ok(result) => Ok(result),
                Err(url_err) => {
                    // Log the URL error and fall back to tweet text
                    logging::log_info(
                        "summary",
                        &format!(
                            "URL summarization failed, falling back to tweet: {}",
                            url_err
                        ),
                    );
                    ai.summarize_streaming(SummaryInput {
                        text: tweet_text,
                        source_url: Some(tweet_url.clone()),
                        title: None,
                    })
                    .await
                }
            }
        } else {
            ai.summarize_streaming(SummaryInput {
                text: tweet_text,
                source_url: Some(tweet_url.clone()),
                title: None,
            })
            .await
        };

        self.ai = Some(ai);

        match result {
            Ok((meta, rx)) => {
                self.summary = Some(SummaryState {
                    text: String::new(),
                    source_url: meta.source_url,
                    provider: meta.provider,
                    model: meta.model,
                });
                self.summary_stream = Some(rx);
            }
            Err(err) => {
                progress::clear();
                self.set_summary_error(format_ai_error(&err, &self.config.ai.provider));
                self.summary_loading = false;
            }
        }
    }

    async fn speak_summary(&mut self) {
        let Some(summary) = &self.summary else {
            self.set_summary_error("No summary to speak");
            return;
        };
        if summary.text.trim().is_empty() {
            self.set_summary_error("No summary to speak");
            return;
        }

        if !self.config.tts.enabled {
            self.set_summary_error("TTS is disabled in config");
            return;
        }

        let summary_text = summary.text.clone();
        let mut tts = self
            .tts
            .take()
            .unwrap_or_else(|| TtsClient::new(self.config.tts.clone()));

        progress::start_indeterminate();
        let result = tts.speak(&summary_text).await;
        progress::clear();

        self.tts = Some(tts);

        if let Err(err) = result {
            self.set_summary_error(format!("TTS failed: {:#}", err));
        }
    }

    /// Get all image URLs for a tweet (photos only)
    pub fn get_tweet_image_urls(&self, tweet: &Tweet) -> Vec<String> {
        tweet.media.iter().filter_map(|m| m.small_url()).collect()
    }

    /// Get larger image URLs for AI analysis (photos only)
    pub fn get_tweet_image_urls_for_ai(&self, tweet: &Tweet) -> Vec<String> {
        tweet.media.iter().filter_map(|m| m.ai_url()).collect()
    }
}

fn filter_summary_urls_for_images(urls: &[String], has_images: bool) -> Vec<String> {
    urls.iter()
        .filter(|url| {
            let Ok(parsed) = Url::parse(url) else {
                return true;
            };
            let host = parsed.domain().unwrap_or_default();
            if (host == "x.com" || host == "twitter.com") && parsed.path().contains("/status/") {
                return false;
            }
            if has_images && host == "t.co" {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

fn build_summary_text(tweet: &Tweet) -> String {
    let mut parts = Vec::new();

    if tweet.is_retweet
        && let Some(retweeter) = tweet.retweeted_by.as_deref()
    {
        parts.push(format!("Retweeted by @{}.", retweeter));
    }

    let base_text = tweet.text.trim();
    if !base_text.is_empty() {
        parts.push(format!(
            "Tweet by @{}:\n{}",
            tweet.user.screen_name, base_text
        ));
    }

    if let Some(quoted) = tweet.quoted_tweet.as_deref() {
        let quoted_text = quoted.text.trim();
        if !quoted_text.is_empty() {
            parts.push(format!(
                "Quoted tweet by @{}:\n{}",
                quoted.user.screen_name, quoted_text
            ));
        }
    }

    parts.join("\n\n")
}

fn merge_newer(existing: &[Tweet], new_tweets: &[Tweet]) -> Vec<Tweet> {
    let mut seen = HashSet::new();
    let mut merged = Vec::with_capacity(existing.len() + new_tweets.len());

    for tweet in new_tweets {
        if seen.insert(tweet.id.clone()) {
            merged.push(tweet.clone());
        }
    }

    for tweet in existing {
        if seen.insert(tweet.id.clone()) {
            merged.push(tweet.clone());
        }
    }

    merged
}

fn append_unique(target: &mut Vec<Tweet>, new_tweets: &[Tweet]) {
    let mut seen: HashSet<String> = target.iter().map(|tweet| tweet.id.clone()).collect();
    for tweet in new_tweets {
        if seen.insert(tweet.id.clone()) {
            target.push(tweet.clone());
        }
    }
}

/// Format AI errors with user-friendly messages and suggestions
fn format_ai_error(err: &anyhow::Error, provider: &AiProvider) -> String {
    let err_str = format!("{:#}", err);
    let err_lower = err_str.to_lowercase();

    // Rate limit errors (429)
    if err_lower.contains("429")
        || err_lower.contains("rate limit")
        || err_lower.contains("quota")
        || err_lower.contains("resource_exhausted")
    {
        // Try to extract retry delay
        let retry_hint = if let Some(pos) = err_str.find("retry in") {
            let after = &err_str[pos..];
            if let Some(end) = after.find('.').or_else(|| after.find(',')) {
                format!(" ({})", &after[..end])
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        return format!(
            "Rate limit exceeded for {}{}\n\n\
            Try: press 'c' to switch to a different AI provider,\n\
            or wait a moment and try again.",
            provider, retry_hint
        );
    }

    // Authentication errors (401, 403)
    if err_lower.contains("401")
        || err_lower.contains("403")
        || err_lower.contains("unauthorized")
        || err_lower.contains("forbidden")
        || err_lower.contains("invalid api key")
    {
        return format!(
            "Authentication failed for {}\n\n\
            Check your API key in config.yaml (ai.api_key)\n\
            or set the appropriate environment variable.",
            provider
        );
    }

    // Timeout errors
    if err_lower.contains("timeout") || err_lower.contains("timed out") {
        return format!(
            "Request timed out for {}\n\n\
            The AI service is slow or unavailable.\n\
            Try again or switch providers with 'c'.",
            provider
        );
    }

    // Context window exceeded (common with FoundationModels/local models)
    if err_lower.contains("context window")
        || err_lower.contains("context length")
        || err_lower.contains("too many tokens")
        || err_lower.contains("maximum context")
    {
        return format!(
            "Content too long for {}\n\n\
            The tweet thread exceeds this model's context limit.\n\
            Try: press 'c' to switch to a provider with larger context\n\
            (Google or OpenAI recommended for long threads).",
            provider
        );
    }

    // Network errors
    if err_lower.contains("connection")
        || err_lower.contains("network")
        || err_lower.contains("dns")
    {
        return format!(
            "Network error connecting to {}\n\n\
            Check your internet connection.",
            provider
        );
    }

    // Default: show the error but add provider context
    format!("{} error: {}", provider, err_str)
}
