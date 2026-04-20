use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use reqwest::Url;
use tokio::sync::mpsc::{self, error::TryRecvError};

use crate::ai::{ImageSummaryInput, SummaryInput, TtsClient, extract_urls};
use crate::config::AiProvider;
use crate::logging;
use crate::tui::app::progress;
use crate::tui::shell::{Shell, ShellBootstrap};
use crate::twitter::Tweet;

#[derive(Debug, Clone)]
pub struct SummaryState {
    pub text: String,
    pub source_url: Option<String>,
    pub provider: AiProvider,
    pub model: String,
}

pub struct TimelineView {
    pub tweets: Vec<Tweet>,
    pub selected: usize,
    pub next_cursor: Option<String>,

    pub loading: bool,
    pub loading_tick: u8,
    pub loading_more: bool,
    pub error: Option<String>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub latest_loaded_at: Option<DateTime<Utc>>,
    pub loaded_from_cache: bool,

    pub summary: Option<SummaryState>,
    pub summary_loading: bool,
    pub summary_error: Option<String>,
    summary_stream: Option<mpsc::Receiver<String>>,
    pending_auto_speak: bool,
    pub summary_scroll: usize,
    summary_area: Option<Rect>,
    summary_viewport_height: u16,
    summary_content_height: usize,

    pub image_scroll: usize,
}

impl TimelineView {
    pub const SUMMARY_SCROLL_LINES: usize = 3;

    pub fn new(bootstrap: ShellBootstrap) -> Self {
        Self {
            tweets: bootstrap.tweets,
            selected: 0,
            next_cursor: None,
            loading: false,
            loading_tick: 0,
            loading_more: false,
            error: None,
            last_refresh: None,
            latest_loaded_at: bootstrap.latest_loaded_at,
            loaded_from_cache: bootstrap.loaded_from_cache,
            summary: None,
            summary_loading: false,
            summary_error: None,
            summary_stream: None,
            pending_auto_speak: false,
            summary_scroll: 0,
            summary_area: None,
            summary_viewport_height: 0,
            summary_content_height: 0,
            image_scroll: 0,
        }
    }

    // -- Queries --

    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }

    pub fn timeline_len(&self) -> usize {
        self.tweets.len() + usize::from(self.has_more())
    }

    pub fn is_load_more_selected(&self) -> bool {
        self.has_more() && self.selected == self.tweets.len()
    }

    pub fn should_auto_load_more(&self) -> bool {
        self.has_more()
            && !self.loading_more
            && self.selected + 3 >= self.tweets.len()
    }

    // -- Errors --

    pub fn set_error(&mut self, context: &str, message: impl Into<String>) {
        let message = message.into();
        self.error = Some(message.clone());
        logging::log_error(context, &message);
    }

    fn set_summary_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.summary_error = Some(message.clone());
        logging::log_error("summary", &message);
    }

    // -- Navigation --

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.image_scroll = 0;
            self.clear_summary();
        }
    }

    pub fn move_down(&mut self) {
        let max_index = self.timeline_len().saturating_sub(1);
        if self.timeline_len() > 0 && self.selected < max_index {
            self.selected += 1;
            self.image_scroll = 0;
            self.clear_summary();
        }
    }

    pub fn move_to_top(&mut self) {
        self.selected = 0;
        self.image_scroll = 0;
        self.clear_summary();
    }

    pub fn move_to_bottom(&mut self) {
        if self.timeline_len() > 0 {
            self.selected = self.timeline_len().saturating_sub(1);
            self.image_scroll = 0;
            self.clear_summary();
        }
    }

    pub fn image_scroll_left(&mut self) {
        if self.image_scroll > 0 {
            self.image_scroll -= 1;
        }
    }

    pub fn image_scroll_right(&mut self) {
        if let Some(tweet) = self.tweets.get(self.selected) {
            let image_count = get_tweet_image_urls(tweet).len();
            if image_count > 0 && self.image_scroll < image_count - 1 {
                self.image_scroll += 1;
            }
        }
    }

    // -- Actions --

    pub fn open_selected(&self) {
        if let Some(tweet) = self.tweets.get(self.selected) {
            let url = tweet.url();
            let _ = open::that(&url);
        }
    }

    // -- Summary panel --

    pub fn clear_summary(&mut self) {
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

    pub fn set_summary_scroll_bounds(
        &mut self,
        content_height: usize,
        viewport_height: u16,
    ) {
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

    pub fn summary_scroll_by(&mut self, delta: i32) {
        let max_scroll = self.summary_max_scroll();
        let new_scroll = self
            .summary_scroll
            .saturating_add_signed(delta as isize)
            .min(max_scroll);
        self.summary_scroll = new_scroll;
    }

    pub fn summary_scroll_page(&mut self, down: bool) {
        let step =
            self.summary_viewport_height.saturating_sub(1).max(1) as i32;
        let delta = if down { step } else { -step };
        self.summary_scroll_by(delta);
    }

    pub fn summary_area_contains(&self, column: u16, row: u16) -> bool {
        self.summary_area.is_some_and(|area| {
            area.contains(ratatui::layout::Position {
                x: column,
                y: row,
            })
        })
    }

    // -- Data loading --

    pub async fn refresh(&mut self, shell: &mut Shell) {
        self.loading = true;
        self.error = None;
        progress::start_indeterminate();

        match shell
            .client
            .get_home_timeline_page(shell.config.tweet_count, None)
            .await
        {
            Ok(page) => {
                let new_tweets = page.tweets;

                let merged = if self.loaded_from_cache {
                    new_tweets.clone()
                } else {
                    merge_newer(&self.tweets, &new_tweets)
                };

                if let Err(e) = shell.cache.save_tweets(&new_tweets) {
                    eprintln!("Failed to cache tweets: {}", e);
                }

                let ttl =
                    Duration::from_secs(shell.config.cache_ttl_mins * 60);
                let _ = shell.cache.clear_old(ttl);

                self.tweets = merged;
                self.next_cursor = page.next_cursor;
                self.last_refresh = Some(Utc::now());
                self.latest_loaded_at =
                    self.tweets.iter().map(|tweet| tweet.created_at).max();
                self.loaded_from_cache = false;
                self.clear_summary();

                if self.selected >= self.timeline_len() {
                    self.selected = 0;
                }

                self.load_all_avatars(shell).await;
            }
            Err(e) => {
                self.set_error("refresh", format!("{:#}", e));
                progress::set_error();
            }
        }

        progress::clear();
        self.loading = false;
    }

    pub async fn load_more(&mut self, shell: &mut Shell) {
        if self.loading_more {
            return;
        }
        let Some(cursor) = self.next_cursor.clone() else {
            return;
        };

        self.loading_more = true;
        progress::start_indeterminate();
        match shell
            .client
            .get_home_timeline_page(
                shell.config.tweet_count,
                Some(&cursor),
            )
            .await
        {
            Ok(page) => {
                let mut older = page.tweets;
                let existing_ids: HashSet<String> =
                    self.tweets.iter().map(|tweet| tweet.id.clone()).collect();
                older.retain(|tweet| !existing_ids.contains(&tweet.id));

                if let Err(e) = shell.cache.save_tweets(&older) {
                    eprintln!("Failed to cache tweets: {}", e);
                }

                append_unique(&mut self.tweets, &older);
                self.next_cursor = page.next_cursor;

                if self.selected >= self.timeline_len() {
                    self.selected = self.timeline_len().saturating_sub(1);
                }

                self.load_all_avatars(shell).await;
            }
            Err(err) => {
                self.set_error("load_more", format!("{:#}", err));
                progress::set_error();
            }
        }

        progress::clear();
        self.loading_more = false;
    }

    // -- Image loading --

    async fn load_all_avatars(&mut self, shell: &mut Shell) {
        if !shell.images_enabled {
            return;
        }
        let Some(image_manager) = shell.image_manager.as_mut() else {
            return;
        };

        let urls: Vec<String> = self
            .tweets
            .iter()
            .map(|t| t.user.avatar_url_bigger())
            .filter(|url| !url.is_empty() && !image_manager.is_in_memory(url))
            .collect();

        if urls.is_empty() {
            return;
        }

        progress::start_indeterminate();

        for url in urls {
            let _ = image_manager.load_image(&url).await;
        }

        progress::clear();
    }

    pub fn pending_image_urls(&self, shell: &Shell) -> Vec<String> {
        let Some(image_manager) = shell
            .image_manager
            .as_ref()
            .filter(|_| shell.images_enabled)
        else {
            return Vec::new();
        };

        use crate::config::MediaPrefetch;

        let mut urls = Vec::new();

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

        if let Some(tweet) = self.tweets.get(self.selected) {
            collect_from_tweet(tweet);
        }

        let (start, end) = match shell.config.media_prefetch {
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

        for (i, tweet) in
            self.tweets.iter().enumerate().skip(start).take(end - start)
        {
            if i != self.selected {
                collect_from_tweet(tweet);
            }
        }

        urls
    }

    pub async fn load_one_image(&self, shell: &mut Shell) -> bool {
        let pending = self.pending_image_urls(shell);
        if let Some(url) = pending.first()
            && let Some(image_manager) = shell.image_manager.as_mut()
        {
            let _ = image_manager.load_image(url).await;
        }
        pending.len() > 1
    }

    // -- Summary / AI --

    pub async fn drain_summary_stream(&mut self, shell: &mut Shell) {
        let Some(rx) = self.summary_stream.as_mut() else {
            if self.pending_auto_speak {
                self.pending_auto_speak = false;
                self.speak_summary(shell).await;
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
                    if shell.config.tts.enabled
                        && shell.config.tts.auto_speak_summaries
                    {
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
            self.speak_summary(shell).await;
        }
    }

    #[allow(clippy::too_many_lines)]
    pub async fn summarize_selected(&mut self, shell: &mut Shell) {
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

        if !shell.config.ai.enabled {
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
                get_tweet_image_urls_for_ai(tweet),
            )
        };

        let urls = extract_urls(&tweet_text);
        let summarize_links = shell.config.ai.summarize_links;
        let summarize_tweets = shell.config.ai.summarize_tweets;
        let summarize_images = shell.config.ai.summarize_images;

        let link_candidates =
            filter_summary_urls_for_images(&urls, !image_urls.is_empty());
        let has_link_to_summarize =
            summarize_links && !link_candidates.is_empty();
        let has_images_to_summarize =
            summarize_images && !image_urls.is_empty();
        let has_tweet_to_summarize =
            summarize_tweets && !tweet_text.trim().is_empty();
        if !has_link_to_summarize
            && !has_images_to_summarize
            && !has_tweet_to_summarize
        {
            self.set_summary_error(
                "No link, tweet, or image to summarize",
            );
            self.summary_loading = false;
            return;
        }

        let will_use_images =
            !has_link_to_summarize && has_images_to_summarize;
        if will_use_images
            && !shell
                .config
                .ai
                .provider
                .supports_image_summaries()
        {
            self.set_summary_error(format!(
                "AI provider {} does not support image summaries yet",
                shell.config.ai.provider
            ));
            self.summary_loading = false;
            return;
        }

        if shell.ai.is_none()
            && let Some(err) = shell.rebuild_ai_client()
        {
            self.set_error("ai_init", err);
        }

        let Some(ai) = shell.ai.take() else {
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
            shell.ai = Some(ai);
            match result {
                Ok(summary) => {
                    self.summary = Some(SummaryState {
                        text: summary.summary,
                        source_url: summary.source_url,
                        provider: summary.provider,
                        model: summary.model,
                    });
                    if shell.config.tts.enabled
                        && shell.config.tts.auto_speak_summaries
                    {
                        self.speak_summary(shell).await;
                    }
                }
                Err(err) => {
                    self.set_summary_error(format_ai_error(
                        &err,
                        &shell.config.ai.provider,
                    ));
                }
            }
            self.summary_loading = false;
            return;
        }

        let result = if has_link_to_summarize {
            match ai
                .summarize_url_streaming(&link_candidates[0])
                .await
            {
                Ok(result) => Ok(result),
                Err(url_err) => {
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

        shell.ai = Some(ai);

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
                self.set_summary_error(format_ai_error(
                    &err,
                    &shell.config.ai.provider,
                ));
                self.summary_loading = false;
            }
        }
    }

    pub async fn speak_summary(&mut self, shell: &mut Shell) {
        let Some(summary) = &self.summary else {
            self.set_summary_error("No summary to speak");
            return;
        };
        if summary.text.trim().is_empty() {
            self.set_summary_error("No summary to speak");
            return;
        }

        if !shell.config.tts.enabled {
            self.set_summary_error("TTS is disabled in config");
            return;
        }

        let summary_text = summary.text.clone();
        let tts_config = shell.config.tts.clone();
        let mut tts = shell
            .tts
            .take()
            .unwrap_or_else(|| TtsClient::new(tts_config));

        progress::start_indeterminate();
        let result = tts.speak(&summary_text).await;
        progress::clear();

        shell.tts = Some(tts);

        if let Err(err) = result {
            self.set_summary_error(format!("TTS failed: {:#}", err));
        }
    }
}

// -- Free functions for image URLs (no view/shell state needed) --

pub fn get_tweet_image_urls(tweet: &Tweet) -> Vec<String> {
    tweet.media.iter().filter_map(|m| m.small_url()).collect()
}

pub fn get_tweet_image_urls_for_ai(tweet: &Tweet) -> Vec<String> {
    tweet.media.iter().filter_map(|m| m.ai_url()).collect()
}

// -- Private helpers --

fn filter_summary_urls_for_images(
    urls: &[String],
    has_images: bool,
) -> Vec<String> {
    urls.iter()
        .filter(|url| {
            let Ok(parsed) = Url::parse(url) else {
                return true;
            };
            let host = parsed.domain().unwrap_or_default();
            if (host == "x.com" || host == "twitter.com")
                && parsed.path().contains("/status/")
            {
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
    let mut merged =
        Vec::with_capacity(existing.len() + new_tweets.len());

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
    let mut seen: HashSet<String> =
        target.iter().map(|tweet| tweet.id.clone()).collect();
    for tweet in new_tweets {
        if seen.insert(tweet.id.clone()) {
            target.push(tweet.clone());
        }
    }
}

fn format_ai_error(err: &anyhow::Error, provider: &AiProvider) -> String {
    let err_str = format!("{:#}", err);
    let err_lower = err_str.to_lowercase();

    if err_lower.contains("429")
        || err_lower.contains("rate limit")
        || err_lower.contains("quota")
        || err_lower.contains("resource_exhausted")
    {
        let retry_hint = if let Some(pos) = err_str.find("retry in") {
            let after = &err_str[pos..];
            if let Some(end) = after.find('.').or_else(|| after.find(','))
            {
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

    if err_lower.contains("timeout")
        || err_lower.contains("timed out")
    {
        return format!(
            "Request timed out for {}\n\n\
            The AI service is slow or unavailable.\n\
            Try again or switch providers with 'c'.",
            provider
        );
    }

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

    format!("{} error: {}", provider, err_str)
}
