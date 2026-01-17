use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::{env, fmt};

use crate::ai::tts::catalog as tts_catalog;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Twitter auth_token cookie
    pub auth_token: String,
    /// Twitter ct0 (CSRF) cookie
    pub ct0: String,
    /// Auto-refresh interval in seconds (0 to disable)
    #[serde(default = "default_refresh_secs")]
    pub refresh_secs: u64,
    /// Number of tweets to fetch per request
    #[serde(default = "default_tweet_count")]
    pub tweet_count: usize,
    /// Whether to show inline images
    #[serde(default = "default_show_images")]
    pub show_images: bool,
    /// Media prefetch strategy (nearby, visible, all)
    #[serde(default)]
    pub media_prefetch: MediaPrefetch,
    /// Maximum image cache size in MB (LRU eviction when exceeded)
    #[serde(default = "default_image_cache_max_mb")]
    pub image_cache_max_mb: u64,
    /// Cache TTL in minutes
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_mins: u64,
    /// Max tweet text width in characters before truncation
    #[serde(default = "default_tweet_max_width")]
    pub tweet_max_width: usize,
    /// Color theme (default, vibrant)
    #[serde(default)]
    pub theme: Theme,
    /// Enable debug logging to ~/.cache/twit/twit.log
    #[serde(default)]
    pub debug: bool,
    /// AI summarization configuration
    #[serde(default)]
    pub ai: AiConfig,
    /// Text-to-speech configuration
    #[serde(default)]
    pub tts: TtsConfig,
}

/// Color theme for the TUI
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// ANSI colors - works well across terminal themes
    #[default]
    Default,
    /// RGB colors - vibrant, requires true color support
    Vibrant,
}

/// Media prefetch strategy
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaPrefetch {
    /// Load media for tweets near the selection (±5 tweets) - for low resource machines
    Nearby,
    /// Load media for all currently visible tweets
    Visible,
    /// Load media for all loaded tweets (default)
    #[default]
    All,
}

#[derive(Debug, Default, Clone)]
pub struct ConfigValidation {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ConfigValidation {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

fn default_refresh_secs() -> u64 {
    300 // 5 minutes
}
fn default_tweet_count() -> usize {
    50
}
fn default_show_images() -> bool {
    true
}
fn default_cache_ttl() -> u64 {
    60 // 1 hour
}
fn default_tweet_max_width() -> usize {
    140
}
fn default_image_cache_max_mb() -> u64 {
    200 // 200 MB default
}
fn default_ai_enabled() -> bool {
    false
}
fn default_ai_model() -> String {
    crate::ai::default_model_for_provider(AiProvider::default()).to_string()
}

fn normalize_ai_model(provider: AiProvider, model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return crate::ai::default_model_for_provider(provider).to_string();
    }

    if provider == AiProvider::FoundationModels {
        return crate::ai::default_model_for_provider(provider).to_string();
    }

    if provider == AiProvider::Google {
        if trimmed.starts_with("models/")
            || trimmed.starts_with("gemini-")
            || trimmed.starts_with("text-embedding")
        {
            return trimmed.to_string();
        }
        return crate::ai::default_model_for_provider(provider).to_string();
    }

    trimmed.to_string()
}
fn default_ai_system_prompt() -> String {
    "You summarize links and tweets for a terminal reader.".to_string()
}
fn default_ai_max_input_chars() -> usize {
    12_000
}
fn default_ai_max_output_tokens() -> u32 {
    0
}
fn default_ai_request_timeout_secs() -> u64 {
    30
}
fn default_ai_max_fetch_bytes() -> usize {
    2_000_000
}
fn default_ai_cache_capacity() -> usize {
    128
}
fn default_ai_summarize_links() -> bool {
    true
}
fn default_ai_summarize_tweets() -> bool {
    true
}
fn default_ai_summarize_images() -> bool {
    true
}
fn default_tts_enabled() -> bool {
    false
}
fn default_tts_auto_speak() -> bool {
    false
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auth_token: String::new(),
            ct0: String::new(),
            refresh_secs: default_refresh_secs(),
            tweet_count: default_tweet_count(),
            show_images: default_show_images(),
            media_prefetch: MediaPrefetch::default(),
            image_cache_max_mb: default_image_cache_max_mb(),
            cache_ttl_mins: default_cache_ttl(),
            tweet_max_width: default_tweet_max_width(),
            theme: Theme::default(),
            debug: false,
            ai: AiConfig::default(),
            tts: TtsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AiProvider {
    #[default]
    #[serde(rename = "openai")]
    OpenAI,
    Anthropic,
    Google,
    #[serde(rename = "openrouter")]
    OpenRouter,
    Copilot,
    Local,
    FoundationModels,
}

impl AiProvider {
    pub fn next(self) -> Self {
        match self {
            AiProvider::OpenAI => AiProvider::Anthropic,
            AiProvider::Anthropic => AiProvider::Google,
            AiProvider::Google => AiProvider::OpenRouter,
            AiProvider::OpenRouter => AiProvider::Local,
            AiProvider::Local => AiProvider::Copilot,
            AiProvider::Copilot => AiProvider::FoundationModels,
            AiProvider::FoundationModels => AiProvider::OpenAI,
        }
    }

    pub fn supports_image_summaries(&self) -> bool {
        matches!(self, AiProvider::OpenAI)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AiProvider::OpenAI => "openai",
            AiProvider::Anthropic => "anthropic",
            AiProvider::Google => "google",
            AiProvider::OpenRouter => "openrouter",
            AiProvider::Copilot => "copilot",
            AiProvider::Local => "local",
            AiProvider::FoundationModels => "foundation-models",
        }
    }
}

impl std::fmt::Display for AiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SummaryLength {
    #[default]
    Short,
    Medium,
    Long,
}

pub fn summary_length_max_tokens(length: SummaryLength) -> u32 {
    match length {
        SummaryLength::Short => 1024,
        SummaryLength::Medium => 2048,
        SummaryLength::Long => 4096,
    }
}

impl fmt::Display for SummaryLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            SummaryLength::Short => "short",
            SummaryLength::Medium => "medium",
            SummaryLength::Long => "long",
        };
        f.write_str(label)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default = "default_ai_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub provider: AiProvider,
    #[serde(default = "default_ai_model")]
    pub model: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_ai_system_prompt")]
    pub system_prompt: String,
    #[serde(default)]
    pub summary_length: SummaryLength,
    #[serde(default = "default_ai_max_input_chars")]
    pub max_input_chars: usize,
    #[serde(default = "default_ai_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_ai_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_ai_max_fetch_bytes")]
    pub max_fetch_bytes: usize,
    #[serde(default = "default_ai_cache_capacity")]
    pub cache_capacity: usize,
    #[serde(default = "default_ai_summarize_links")]
    pub summarize_links: bool,
    #[serde(default = "default_ai_summarize_tweets")]
    pub summarize_tweets: bool,
    #[serde(default = "default_ai_summarize_images")]
    pub summarize_images: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: default_ai_enabled(),
            provider: AiProvider::default(),
            model: default_ai_model(),
            api_base: None,
            api_key: None,
            system_prompt: default_ai_system_prompt(),
            summary_length: SummaryLength::default(),
            max_input_chars: default_ai_max_input_chars(),
            max_output_tokens: default_ai_max_output_tokens(),
            request_timeout_secs: default_ai_request_timeout_secs(),
            max_fetch_bytes: default_ai_max_fetch_bytes(),
            cache_capacity: default_ai_cache_capacity(),
            summarize_links: default_ai_summarize_links(),
            summarize_tweets: default_ai_summarize_tweets(),
            summarize_images: default_ai_summarize_images(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TtsProvider {
    #[default]
    Say,
    #[serde(rename = "openai")]
    OpenAI,
    Google,
    #[serde(rename = "elevenlabs")]
    ElevenLabs,
}

impl std::fmt::Display for TtsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            TtsProvider::Say => "say",
            TtsProvider::OpenAI => "openai",
            TtsProvider::Google => "google",
            TtsProvider::ElevenLabs => "elevenlabs",
        };
        f.write_str(label)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    #[serde(default = "default_tts_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub provider: TtsProvider,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub rate_wpm: Option<u32>,
    #[serde(default = "default_tts_auto_speak")]
    pub auto_speak_summaries: bool,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: default_tts_enabled(),
            provider: TtsProvider::default(),
            model: None,
            api_base: None,
            api_key: None,
            voice: None,
            rate_wpm: None,
            auto_speak_summaries: default_tts_auto_speak(),
        }
    }
}

impl Config {
    /// Get the project directories (XDG on Linux, standard locations on macOS/Windows)
    fn project_dirs() -> Result<ProjectDirs> {
        ProjectDirs::from("io", "blacktop", "twit").context("Failed to determine config directory")
    }

    /// Get the config file path (~/.config/twit/config.yaml on Linux)
    pub fn config_path() -> PathBuf {
        Self::project_dirs()
            .map(|dirs| dirs.config_dir().join("config.yaml"))
            .unwrap_or_else(|_| PathBuf::from("config.yaml"))
    }

    /// Get the cache directory (~/.cache/twit on Linux)
    pub fn cache_dir() -> PathBuf {
        Self::project_dirs()
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .unwrap_or_else(|_| PathBuf::from(".cache"))
    }

    /// Get the log file path (~/.cache/twit/twit.log on Linux)
    pub fn log_path() -> PathBuf {
        Self::cache_dir().join("twit.log")
    }

    /// Load config from disk
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;
        let mut config: Config = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {}", path.display()))?;

        let provider = config.ai.provider.clone();
        config.ai.model = normalize_ai_model(provider, &config.ai.model);

        if config.auth_token.is_empty() || config.ct0.is_empty() {
            anyhow::bail!("Config is missing auth_token or ct0");
        }

        Ok(config)
    }

    pub fn validate(&self) -> ConfigValidation {
        let mut report = ConfigValidation::default();

        if self.auth_token.trim().is_empty() {
            report.errors.push("auth_token is missing".to_string());
        }
        if self.ct0.trim().is_empty() {
            report.errors.push("ct0 is missing".to_string());
        }
        if self.tweet_count == 0 {
            report
                .errors
                .push("tweet_count must be greater than 0".to_string());
        }
        if self.image_cache_max_mb == 0 && self.show_images {
            report.warnings.push(
                "image_cache_max_mb is 0; disk caching of images is effectively disabled"
                    .to_string(),
            );
        }

        if self.ai.enabled {
            if self.ai.max_fetch_bytes == 0 {
                report
                    .errors
                    .push("ai.max_fetch_bytes must be greater than 0".to_string());
            }
            let recommended_tokens = summary_length_max_tokens(self.ai.summary_length);
            if self.ai.max_output_tokens > 0 && self.ai.max_output_tokens < recommended_tokens {
                report.warnings.push(format!(
                    "ai.max_output_tokens ({}) is lower than recommended for summary_length {} ({}); summaries may be shorter than expected",
                    self.ai.max_output_tokens, self.ai.summary_length, recommended_tokens
                ));
            }
            if !self.ai.summarize_links && !self.ai.summarize_tweets && !self.ai.summarize_images {
                report.warnings.push(
                    "ai.enabled is true but summarize_links, summarize_tweets, and summarize_images are all false"
                        .to_string(),
                );
            }

            match self.ai.provider {
                AiProvider::OpenAI => {
                    if !has_config_or_env_key(&self.ai.api_key, "OPENAI_API_KEY") {
                        report.errors.push(
                            "OpenAI API key missing (set ai.api_key or OPENAI_API_KEY)".to_string(),
                        );
                    }
                }
                AiProvider::Google => {
                    if !has_config_or_env_key(&self.ai.api_key, "GEMINI_API_KEY") {
                        report.errors.push(
                            "Gemini API key missing (set ai.api_key or GEMINI_API_KEY)".to_string(),
                        );
                    }
                }
                AiProvider::FoundationModels => {
                    if !cfg!(target_os = "macos") {
                        report.errors.push(
                            "AI provider 'foundation-models' is only available on macOS"
                                .to_string(),
                        );
                    }
                }
                _ => {
                    report.warnings.push(format!(
                        "AI provider '{}' is not available in this release",
                        self.ai.provider
                    ));
                }
            }
        }

        if self.tts.enabled {
            match self.tts.provider {
                TtsProvider::Say => {
                    if !cfg!(target_os = "macos") {
                        report
                            .errors
                            .push("TTS provider 'say' is only available on macOS".to_string());
                    }
                }
                TtsProvider::OpenAI => {
                    if !has_config_or_env_key(&self.tts.api_key, "OPENAI_API_KEY") {
                        report.errors.push(
                            "OpenAI TTS API key missing (set tts.api_key or OPENAI_API_KEY)"
                                .to_string(),
                        );
                    }
                    if let Some(model) = self
                        .tts
                        .model
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        && !tts_catalog::is_openai_model(model)
                    {
                        report.warnings.push(format!(
                            "OpenAI TTS model '{}' is not in the known catalog",
                            model
                        ));
                    }
                    if let Some(voice) = self
                        .tts
                        .voice
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        && !tts_catalog::is_openai_voice(voice)
                    {
                        report.warnings.push(format!(
                            "OpenAI TTS voice '{}' is not in the known catalog",
                            voice
                        ));
                    }
                }
                TtsProvider::Google => {
                    if !has_config_or_env_key(&self.tts.api_key, "GEMINI_API_KEY") {
                        report.errors.push(
                            "Gemini TTS API key missing (set tts.api_key or GEMINI_API_KEY)"
                                .to_string(),
                        );
                    }
                    if let Some(model) = self
                        .tts
                        .model
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let normalized = model.strip_prefix("models/").unwrap_or(model);
                        if !tts_catalog::is_google_model(normalized) {
                            report.warnings.push(format!(
                                "Gemini TTS model '{}' is not in the known catalog",
                                model
                            ));
                        }
                    }
                    if let Some(voice) = self
                        .tts
                        .voice
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        && !tts_catalog::is_google_voice(voice)
                    {
                        report.warnings.push(format!(
                            "Gemini TTS voice '{}' is not in the known catalog",
                            voice
                        ));
                    }
                }
                TtsProvider::ElevenLabs => {
                    if !has_config_or_env_key(&self.tts.api_key, "ELEVENLABS_API_KEY")
                        && !has_config_or_env_key(&self.tts.api_key, "XI_API_KEY")
                    {
                        report.errors.push(
                            "ElevenLabs API key missing (set tts.api_key or ELEVENLABS_API_KEY)"
                                .to_string(),
                        );
                    }
                }
            }

            if self.tts.auto_speak_summaries && !self.ai.enabled {
                report
                    .warnings
                    .push("tts.auto_speak_summaries is true but ai.enabled is false".to_string());
            }
        }

        report
    }

    /// Save config to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let contents = serde_yaml::to_string(self).context("Failed to serialize config")?;
        fs::write(&path, contents)
            .with_context(|| format!("Failed to write config to {}", path.display()))?;

        Ok(())
    }
}

fn has_config_or_env_key(config_key: &Option<String>, env_key: &str) -> bool {
    config_key.as_deref().is_some_and(|v| !v.trim().is_empty())
        || env::var(env_key).is_ok_and(|v| !v.trim().is_empty())
}

impl fmt::Display for ConfigValidation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for warning in &self.warnings {
            writeln!(f, "Warning: {}", warning)?;
        }
        for error in &self.errors {
            writeln!(f, "Error: {}", error)?;
        }
        Ok(())
    }
}
