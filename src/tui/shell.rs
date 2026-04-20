use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::ai::{AiClient, TtsClient};
use crate::config::Config;
use crate::logging;
use crate::twitter::{Tweet, TweetCache, TwitterClient};
use crate::widgets::ImageManager;

/// Data produced by Shell::new for TimelineView bootstrap
pub struct ShellBootstrap {
    pub tweets: Vec<Tweet>,
    pub latest_loaded_at: Option<DateTime<Utc>>,
    pub loaded_from_cache: bool,
}

/// Shared resources available to all views
pub struct Shell {
    pub config: Config,
    pub client: TwitterClient,
    pub cache: TweetCache,
    pub ai: Option<AiClient>,
    pub tts: Option<TtsClient>,
    pub image_manager: Option<ImageManager>,
    pub images_enabled: bool,
}

impl Shell {
    pub async fn new(mut config: Config) -> Result<(Self, ShellBootstrap)> {
        let client =
            TwitterClient::new(config.auth_token.clone(), config.ct0.clone())?;
        let cache = TweetCache::open()?;

        let tweets = cache.get_tweets().unwrap_or_default();
        let latest_loaded_at = cache
            .latest_tweet_time()
            .ok()
            .flatten()
            .or_else(|| tweets.iter().map(|tweet| tweet.created_at).max());
        let loaded_from_cache = !tweets.is_empty();

        let (image_manager, images_enabled) = if config.show_images {
            match ImageManager::new_with_cache_limit(config.image_cache_max_mb) {
                Ok(mgr) => (Some(mgr), true),
                Err(_) => (None, false),
            }
        } else {
            (None, false)
        };

        let ai = if config.ai.enabled {
            match AiClient::new(config.ai.clone()) {
                Ok(ai_client) => Some(ai_client),
                Err(err) => {
                    let message = format!("AI init failed: {}", err);
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

        let bootstrap = ShellBootstrap {
            tweets,
            latest_loaded_at,
            loaded_from_cache,
        };

        Ok((
            Self {
                config,
                client,
                cache,
                ai,
                tts,
                image_manager,
                images_enabled,
            },
            bootstrap,
        ))
    }

    /// Cycle to the next AI provider; returns error message if init fails
    pub fn cycle_ai_provider(&mut self) -> Option<String> {
        self.config.ai.provider = self.config.ai.provider.clone().next();
        if self.config.ai.enabled {
            self.rebuild_ai_client()
        } else {
            None
        }
    }

    /// Rebuild the AI client; returns error message on failure
    pub fn rebuild_ai_client(&mut self) -> Option<String> {
        match AiClient::new(self.config.ai.clone()) {
            Ok(client) => {
                self.ai = Some(client);
                None
            }
            Err(err) => {
                let message = format!("AI init failed: {}", err);
                logging::log_error("ai_init", &message);
                self.ai = None;
                self.config.ai.enabled = false;
                Some(message)
            }
        }
    }

    pub fn toggle_images(&mut self) {
        self.images_enabled = !self.images_enabled;
        if self.images_enabled && self.image_manager.is_none() {
            self.image_manager =
                ImageManager::new_with_cache_limit(self.config.image_cache_max_mb)
                    .ok();
        }
    }
}
