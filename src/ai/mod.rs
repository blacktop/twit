use anyhow::{Context, Result};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use unicode_segmentation::UnicodeSegmentation;

use crate::config::{AiConfig, AiProvider, SummaryLength, summary_length_max_tokens};

pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5-nano-2025-08-07";
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
pub const DEFAULT_GOOGLE_MODEL: &str = "gemini-3-flash-preview";
pub const DEFAULT_OPENROUTER_MODEL: &str = "openrouter/auto";
pub const DEFAULT_COPILOT_MODEL: &str = "claude-haiku-4-5";
pub const DEFAULT_LOCAL_MODEL: &str = "gemma3";
pub const DEFAULT_FOUNDATION_MODEL: &str = "system";

pub fn default_model_for_provider(provider: AiProvider) -> &'static str {
    match provider {
        AiProvider::OpenAI => DEFAULT_OPENAI_MODEL,
        AiProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        AiProvider::Google => DEFAULT_GOOGLE_MODEL,
        AiProvider::OpenRouter => DEFAULT_OPENROUTER_MODEL,
        AiProvider::Copilot => DEFAULT_COPILOT_MODEL,
        AiProvider::Local => DEFAULT_LOCAL_MODEL,
        AiProvider::FoundationModels => DEFAULT_FOUNDATION_MODEL,
    }
}

mod content;
mod foundation_models;
mod google;
mod openai;
pub(crate) mod tts;

pub use content::{ExtractedContent, extract_urls};
pub use tts::TtsClient;

#[derive(Debug, Clone)]
pub struct SummaryInput {
    pub text: String,
    pub source_url: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImageSummaryInput {
    pub tweet_text: Option<String>,
    pub image_urls: Vec<String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SummaryOutput {
    pub summary: String,
    pub provider: AiProvider,
    pub model: String,
    pub source_url: Option<String>,
}

/// Metadata for streaming summary (without the text itself)
#[derive(Debug, Clone)]
pub struct SummaryOutputMeta {
    pub provider: AiProvider,
    pub model: String,
    pub source_url: Option<String>,
}

pub struct AiClient {
    http: reqwest::Client,
    config: AiConfig,
    content_cache: Option<Mutex<LruCache<ContentCacheKey, ExtractedContent>>>,
}

impl AiClient {
    pub fn new(config: AiConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("twit-ai/0.1")
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(10))
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .https_only(true)
            .build()
            .context("Failed to build AI HTTP client")?;

        let content_cache =
            NonZeroUsize::new(config.cache_capacity).map(|cap| Mutex::new(LruCache::new(cap)));

        Ok(Self {
            http,
            config,
            content_cache,
        })
    }

    pub async fn summarize(&self, input: SummaryInput) -> Result<SummaryOutput> {
        let truncated_text = truncate_graphemes(&input.text, self.config.max_input_chars);
        let was_truncated =
            truncated_text.graphemes(true).count() < input.text.graphemes(true).count();
        let summary = match self.config.provider {
            AiProvider::OpenAI => {
                let request = SummaryInput {
                    text: truncated_text,
                    source_url: input.source_url.clone(),
                    title: input.title.clone(),
                };
                openai::summarize(&self.http, &self.config, &request, was_truncated).await
            }
            AiProvider::FoundationModels => {
                let request = SummaryInput {
                    text: truncated_text,
                    source_url: input.source_url.clone(),
                    title: input.title.clone(),
                };
                foundation_models::summarize(&self.config, &request, was_truncated).await
            }
            AiProvider::Google => {
                let request = SummaryInput {
                    text: truncated_text,
                    source_url: input.source_url.clone(),
                    title: input.title.clone(),
                };
                google::summarize(&self.config, &request, was_truncated).await
            }
            AiProvider::Anthropic
            | AiProvider::OpenRouter
            | AiProvider::Copilot
            | AiProvider::Local => {
                anyhow::bail!(
                    "AI provider '{}' is not available in this release",
                    self.config.provider
                )
            }
        }?;

        Ok(SummaryOutput {
            summary,
            provider: self.config.provider.clone(),
            model: provider_model_label(&self.config),
            source_url: input.source_url,
        })
    }

    /// Streaming summarization - returns metadata and a channel that yields text chunks.
    /// Only supported for Google provider; others fall back to non-streaming.
    pub async fn summarize_streaming(
        &self,
        input: SummaryInput,
    ) -> Result<(SummaryOutputMeta, mpsc::Receiver<String>)> {
        let truncated_text = truncate_graphemes(&input.text, self.config.max_input_chars);
        let was_truncated =
            truncated_text.graphemes(true).count() < input.text.graphemes(true).count();

        match self.config.provider {
            AiProvider::Google => {
                let request = SummaryInput {
                    text: truncated_text,
                    source_url: input.source_url.clone(),
                    title: input.title.clone(),
                };
                let rx = google::summarize_streaming(&self.config, &request, was_truncated).await?;
                let meta = SummaryOutputMeta {
                    provider: self.config.provider.clone(),
                    model: provider_model_label(&self.config),
                    source_url: input.source_url,
                };
                Ok((meta, rx))
            }
            AiProvider::FoundationModels => {
                let request = SummaryInput {
                    text: truncated_text,
                    source_url: input.source_url.clone(),
                    title: input.title.clone(),
                };
                let rx =
                    foundation_models::summarize_streaming(&self.config, &request, was_truncated)
                        .await?;
                let meta = SummaryOutputMeta {
                    provider: self.config.provider.clone(),
                    model: provider_model_label(&self.config),
                    source_url: input.source_url,
                };
                Ok((meta, rx))
            }
            // Other providers: fall back to non-streaming, send full response at once
            _ => {
                let output = self.summarize(input).await?;
                let (tx, rx) = mpsc::channel(1);
                let _ = tx.send(output.summary).await;
                let meta = SummaryOutputMeta {
                    provider: output.provider,
                    model: output.model,
                    source_url: output.source_url,
                };
                Ok((meta, rx))
            }
        }
    }

    pub async fn summarize_url_streaming(
        &self,
        url: &str,
    ) -> Result<(SummaryOutputMeta, mpsc::Receiver<String>)> {
        let content = self.get_or_fetch_content(url).await?;
        let input = SummaryInput {
            text: content.text,
            source_url: Some(content.url),
            title: content.title,
        };
        self.summarize_streaming(input).await
    }

    pub async fn summarize_images(&self, input: ImageSummaryInput) -> Result<SummaryOutput> {
        let (tweet_text, was_truncated) = match input.tweet_text.as_deref() {
            Some(text) => {
                let truncated = truncate_graphemes(text, self.config.max_input_chars);
                let truncated_flag =
                    truncated.graphemes(true).count() < text.graphemes(true).count();
                (Some(truncated), truncated_flag)
            }
            None => (None, false),
        };

        let image_input = ImageSummaryInput {
            tweet_text,
            image_urls: input.image_urls,
            source_url: input.source_url.clone(),
        };

        let summary = match self.config.provider {
            AiProvider::OpenAI => {
                openai::summarize_images(&self.http, &self.config, &image_input, was_truncated)
                    .await
            }
            AiProvider::FoundationModels => {
                foundation_models::summarize_images(&self.config, &image_input, was_truncated).await
            }
            AiProvider::Anthropic
            | AiProvider::Google
            | AiProvider::OpenRouter
            | AiProvider::Copilot
            | AiProvider::Local => {
                anyhow::bail!(
                    "Image summarization for '{}' is not available in this release",
                    self.config.provider
                )
            }
        }?;

        Ok(SummaryOutput {
            summary,
            provider: self.config.provider.clone(),
            model: provider_model_label(&self.config),
            source_url: input.source_url,
        })
    }

    async fn get_or_fetch_content(&self, url: &str) -> Result<ExtractedContent> {
        let model = self.config.model.clone();
        if let Some(cache) = &self.content_cache {
            let mut cache_guard = cache.lock().await;
            let key = ContentCacheKey {
                url: url.to_string(),
                model: model.clone(),
            };
            if let Some(hit) = cache_guard.get(&key) {
                return Ok(hit.clone());
            }
        }

        let (final_url, html) =
            content::fetch_html(&self.http, url, self.config.max_fetch_bytes).await?;

        // Reject Twitter/X URLs after redirect - they don't have article content
        if is_twitter_url(&final_url) {
            anyhow::bail!(
                "URL redirected to Twitter/X ({}), which doesn't have article content to summarize",
                final_url
            );
        }

        let mut extracted = content::extract_content(&html, &final_url)?;

        // Prefer the final URL so summaries link to the expanded destination.
        extracted.url = final_url.clone();

        if let Some(cache) = &self.content_cache {
            let mut cache_guard = cache.lock().await;
            let final_key = ContentCacheKey {
                url: final_url.clone(),
                model: model.clone(),
            };
            cache_guard.put(final_key, extracted.clone());
            if final_url != url {
                let alias_key = ContentCacheKey {
                    url: url.to_string(),
                    model,
                };
                cache_guard.put(alias_key, extracted.clone());
            }
        }

        Ok(extracted)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ContentCacheKey {
    url: String,
    model: String,
}

fn is_twitter_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default();
    host == "twitter.com"
        || host == "www.twitter.com"
        || host == "x.com"
        || host == "www.x.com"
        || host == "mobile.twitter.com"
        || host == "mobile.x.com"
}

fn truncate_graphemes(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    text.graphemes(true).take(max_chars).collect()
}

fn provider_model_label(config: &AiConfig) -> String {
    match config.provider {
        AiProvider::FoundationModels => "system".to_string(),
        _ => config.model.clone(),
    }
}

struct SummaryLengthSpec {
    guidance: &'static str,
    formatting: &'static str,
    target_chars: usize,
    min_chars: usize,
    max_chars: usize,
}

fn summary_length_spec(length: SummaryLength) -> SummaryLengthSpec {
    match length {
        SummaryLength::Short => SummaryLengthSpec {
            guidance: "Write a tight summary that delivers the primary claim plus one high-signal supporting detail.",
            formatting: "Use 1-2 short paragraphs (a single paragraph is fine). Aim for 2-5 sentences total.",
            target_chars: 900,
            min_chars: 600,
            max_chars: 1200,
        },
        SummaryLength::Medium => SummaryLengthSpec {
            guidance: "Write a clear summary that covers the core claim plus the most important supporting evidence or data points.",
            formatting: "Use 1-3 short paragraphs (2 is typical, but a single paragraph is okay if the content is simple). Aim for 2-3 sentences per paragraph.",
            target_chars: 1800,
            min_chars: 1200,
            max_chars: 2500,
        },
        SummaryLength::Long => SummaryLengthSpec {
            guidance: "Write a detailed summary that prioritizes the most important points first, followed by key supporting facts or events, then secondary details or conclusions stated in the source.",
            formatting: "Paragraphs are optional; use up to 3 short paragraphs. Aim for 2-4 sentences per paragraph when you split into paragraphs.",
            target_chars: 4200,
            min_chars: 2500,
            max_chars: 6000,
        },
    }
}

fn format_length_guidance(spec: &SummaryLengthSpec) -> String {
    format!(
        "Target length: around {} characters (acceptable range {}-{}). This is a soft guideline; prioritize clarity.",
        spec.target_chars, spec.min_chars, spec.max_chars
    )
}

fn build_tagged_prompt(instructions: &str, context: &str, content: &str) -> String {
    let safe_instructions = instructions.trim();
    let safe_context = context.trim();
    let safe_content = content.trim();
    format!(
        "<instructions>\n{}\n</instructions>\n\n<context>\n{}\n</context>\n\n<content>\n{}\n</content>\n",
        safe_instructions, safe_context, safe_content
    )
}

pub(crate) fn build_user_prompt(
    input: &SummaryInput,
    config: &AiConfig,
    truncated: bool,
) -> String {
    let spec = summary_length_spec(config.summary_length);
    let length_guidance = format_length_guidance(&spec);
    let content_chars = input.text.chars().count();
    let content_length_line = if content_chars > 0 {
        format!(
            "Extracted content length: {} characters. Hard limit: never exceed this length. If the requested length is larger, do not pad—finish early rather than adding filler.",
            content_chars
        )
    } else {
        String::new()
    };

    let mut context_lines = Vec::new();
    if let Some(url) = input.source_url.as_deref() {
        context_lines.push(format!("Source URL: {}", url));
    }
    if let Some(title) = input.title.as_deref()
        && !title.trim().is_empty()
    {
        context_lines.push(format!("Title: {}", title.trim()));
    }
    if truncated {
        context_lines.push("Note: Content truncated to the first portion available.".to_string());
    }
    let context = context_lines.join("\n");

    let instructions = [
        "You summarize links and tweets for curious Twitter users who want the gist before deciding to dive in.",
        spec.guidance,
        spec.formatting,
        &length_guidance,
        &content_length_line,
        "Keep the response compact by avoiding blank lines between sentences or list items; use only the single newlines required by the formatting instructions.",
        "Do not use emojis, disclaimers, or speculation.",
        "Write in direct, factual language.",
        "Format the answer in Markdown and obey the length-specific formatting above.",
        "Use short paragraphs; use bullet lists only when they improve scanability; avoid rigid templates.",
        "Base everything strictly on the provided content and never invent details.",
        "Return only the summary.",
    ]
    .iter()
    .filter(|line| !line.trim().is_empty())
    .cloned()
    .collect::<Vec<_>>()
    .join("\n");

    build_tagged_prompt(&instructions, &context, &input.text)
}

pub(crate) fn build_image_prompt(
    tweet_text: Option<&str>,
    config: &AiConfig,
    image_count: usize,
    truncated: bool,
) -> String {
    let spec = summary_length_spec(config.summary_length);
    let length_guidance = format_length_guidance(&spec);
    let mut context_lines = Vec::new();

    if let Some(text) = tweet_text {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            context_lines.push(format!("Tweet text: {}", trimmed));
        }
    }

    context_lines.push(format!("Image count: {}", image_count));

    if truncated {
        context_lines.push("Note: Tweet text truncated.".to_string());
    }

    let context = context_lines.join("\n");

    let instructions = [
        "You describe images attached to tweets for a terminal reader.",
        "Summarize what is visually present: people, objects, actions, text in the image, and notable context.",
        "If multiple images are provided, first summarize the overall scene(s), then mention key differences per image.",
        spec.guidance,
        spec.formatting,
        &length_guidance,
        "Keep the response compact by avoiding blank lines between sentences or list items; use only the single newlines required by the formatting instructions.",
        "Do not use emojis, disclaimers, or speculation.",
        "Write in direct, factual language.",
        "Format the answer in Markdown and obey the length-specific formatting above.",
        "Use short paragraphs; use bullet lists only when they improve scanability; avoid rigid templates.",
        "Base everything strictly on the provided images and tweet text; never invent details.",
        "Return only the summary.",
    ]
    .iter()
    .filter(|line| !line.trim().is_empty())
    .cloned()
    .collect::<Vec<_>>()
    .join("\n");

    build_tagged_prompt(&instructions, &context, "")
}

pub(crate) fn effective_system_prompt(config: &AiConfig) -> String {
    config.system_prompt.trim().to_string()
}

pub(crate) fn effective_max_output_tokens(config: &AiConfig) -> u32 {
    let recommended = summary_length_max_tokens(config.summary_length);
    if config.max_output_tokens == 0 {
        recommended
    } else {
        config.max_output_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SummaryLength;

    #[test]
    fn build_user_prompt_includes_context_and_length_guidance() {
        let config = AiConfig {
            summary_length: SummaryLength::Short,
            ..Default::default()
        };

        let input = SummaryInput {
            text: "Example content body.".to_string(),
            source_url: Some("https://example.com/post".to_string()),
            title: Some("Example Title".to_string()),
        };

        let prompt = build_user_prompt(&input, &config, true);

        assert!(prompt.contains("<instructions>"));
        assert!(prompt.contains("<context>"));
        assert!(prompt.contains("<content>"));
        assert!(prompt.contains("Source URL: https://example.com/post"));
        assert!(prompt.contains("Title: Example Title"));
        assert!(prompt.contains("Target length: around 900 characters"));
        assert!(prompt.contains("Note: Content truncated"));
    }

    #[test]
    fn build_image_prompt_includes_context() {
        let config = AiConfig {
            summary_length: SummaryLength::Short,
            ..Default::default()
        };

        let prompt =
            build_image_prompt(Some("Photo from the conference stage."), &config, 2, false);

        assert!(prompt.contains("<instructions>"));
        assert!(prompt.contains("Image count: 2"));
        assert!(prompt.contains("Tweet text: Photo from the conference stage."));
        assert!(prompt.contains("Target length: around 900 characters"));
    }
}
