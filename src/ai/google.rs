use anyhow::{Context, Result};
use futures::TryStreamExt;
use gemini_rust::{
    GeminiBuilder, GenerationConfig, GenerationResponse, Model, Part, ThinkingConfig, ThinkingLevel,
};
use reqwest::ClientBuilder;
use std::env;
use std::time::Duration;
use tokio::sync::mpsc;
use url::Url;

use crate::ai::{
    SummaryInput, build_user_prompt, default_model_for_provider, effective_max_output_tokens,
    effective_system_prompt,
};
use crate::config::{AiConfig, AiProvider};

pub async fn summarize(config: &AiConfig, input: &SummaryInput, truncated: bool) -> Result<String> {
    let api_key = config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| env::var("GEMINI_API_KEY").ok())
        .context("Gemini API key is missing (set ai.api_key or GEMINI_API_KEY)")?;

    let model = parse_model(&config.model);
    let mut builder = GeminiBuilder::new(api_key)
        .with_model(model)
        .with_http_client(
            ClientBuilder::new()
                .user_agent("twit-ai/0.1")
                .timeout(Duration::from_secs(config.request_timeout_secs))
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .https_only(true),
        );

    if let Some(base_url) = config.api_base.as_deref() {
        let url = Url::parse(base_url.trim())
            .with_context(|| format!("Invalid Gemini base URL '{}'", base_url))?;
        builder = builder.with_base_url(url);
    }

    let client = builder.build().context("Failed to build Gemini client")?;

    let system_prompt = effective_system_prompt(config);
    let user_prompt = build_user_prompt(input, config, truncated);
    let configured_max_output_tokens = config.max_output_tokens;
    let max_output_tokens = effective_max_output_tokens(config);
    let model_label = effective_model_label(config);
    let thinking_config = thinking_config_for_model(model_label);

    let mut attempt_max = max_output_tokens;
    let mut attempts = 0;
    let response = loop {
        let generation_config = GenerationConfig {
            max_output_tokens: Some(attempt_max as i32),
            thinking_config: thinking_config.clone(),
            ..Default::default()
        };
        let response = client
            .generate_content()
            .with_system_prompt(system_prompt.clone())
            .with_user_message(user_prompt.clone())
            .with_generation_config(generation_config)
            .execute()
            .await
            .context("Gemini request failed")?;

        if response_hit_max_tokens(&response) && attempts < 2 && configured_max_output_tokens == 0 {
            attempts += 1;
            attempt_max = attempt_max.saturating_mul(2).min(8192);
            continue;
        }

        break response;
    };

    // Try library's built-in text() first, then manual extraction
    let builtin_text = response.text();
    let summary = if !builtin_text.trim().is_empty() {
        builtin_text
    } else {
        extract_response_text(&response).ok_or_else(|| {
            anyhow::anyhow!(
                "Gemini response missing output text ({})",
                response_diagnostics(&response)
            )
        })?
    };

    Ok(summary)
}

/// Streaming summarization - sends text chunks as they arrive.
/// Returns a receiver channel that yields text chunks and completes when done.
pub async fn summarize_streaming(
    config: &AiConfig,
    input: &SummaryInput,
    truncated: bool,
) -> Result<mpsc::Receiver<String>> {
    let api_key = config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| env::var("GEMINI_API_KEY").ok())
        .context("Gemini API key is missing (set ai.api_key or GEMINI_API_KEY)")?;

    let model = parse_model(&config.model);
    let mut builder = GeminiBuilder::new(api_key)
        .with_model(model)
        .with_http_client(
            ClientBuilder::new()
                .user_agent("twit-ai/0.1")
                .timeout(Duration::from_secs(config.request_timeout_secs))
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .https_only(true),
        );

    if let Some(base_url) = config.api_base.as_deref() {
        let url = Url::parse(base_url.trim())
            .with_context(|| format!("Invalid Gemini base URL '{}'", base_url))?;
        builder = builder.with_base_url(url);
    }

    let client = builder.build().context("Failed to build Gemini client")?;

    let system_prompt = effective_system_prompt(config);
    let user_prompt = build_user_prompt(input, config, truncated);
    let max_output_tokens = effective_max_output_tokens(config);
    let model_label = effective_model_label(config);
    let thinking_config = thinking_config_for_model(model_label);

    let generation_config = GenerationConfig {
        max_output_tokens: Some(max_output_tokens as i32),
        thinking_config,
        ..Default::default()
    };

    let mut stream = client
        .generate_content()
        .with_system_prompt(system_prompt)
        .with_user_message(user_prompt)
        .with_generation_config(generation_config)
        .execute_stream()
        .await
        .context("Gemini streaming request failed")?;

    let (tx, rx) = mpsc::channel::<String>(32);

    // Spawn task to read stream and send chunks
    tokio::spawn(async move {
        while let Ok(Some(chunk)) = stream.try_next().await {
            let text = chunk.text();
            if !text.is_empty() {
                // Ignore send errors (receiver dropped)
                let _ = tx.send(text).await;
            }
        }
    });

    Ok(rx)
}

fn parse_model(value: &str) -> Model {
    let trimmed = value.trim();
    let base = if trimmed.is_empty() {
        default_model_for_provider(AiProvider::Google)
    } else if trimmed.starts_with("models/") {
        return Model::Custom(trimmed.to_string());
    } else if trimmed.starts_with("gemini-") || trimmed.starts_with("text-embedding") {
        trimmed
    } else {
        default_model_for_provider(AiProvider::Google)
    };

    let normalized = if base.starts_with("models/") {
        base.to_string()
    } else {
        format!("models/{}", base)
    };

    Model::Custom(normalized)
}

fn effective_model_label(config: &AiConfig) -> &str {
    let trimmed = config.model.trim();
    if trimmed.is_empty() {
        default_model_for_provider(AiProvider::Google)
    } else {
        trimmed
    }
}

fn thinking_config_for_model(model: &str) -> Option<ThinkingConfig> {
    let normalized = model.strip_prefix("models/").unwrap_or(model);
    // Only configure thinking for gemini-3 models; let other models use defaults
    if normalized.starts_with("gemini-3") {
        return Some(ThinkingConfig {
            thinking_budget: None,
            include_thoughts: Some(false),
            thinking_level: Some(ThinkingLevel::Low),
        });
    }
    // Don't set thinking_config for other models - let API use defaults
    None
}

fn extract_response_text(response: &GenerationResponse) -> Option<String> {
    let mut chunks = Vec::new();
    let mut thought_chunks = Vec::new();
    for candidate in &response.candidates {
        if let Some(parts) = &candidate.content.parts {
            for part in parts {
                if let Part::Text { text, thought, .. } = part {
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if thought.unwrap_or(false) {
                        thought_chunks.push(trimmed.to_string());
                    } else {
                        chunks.push(trimmed.to_string());
                    }
                }
            }
        }
    }

    if !chunks.is_empty() {
        return Some(chunks.join("\n"));
    }
    if !thought_chunks.is_empty() {
        return Some(thought_chunks.join("\n"));
    }
    None
}

fn response_diagnostics(response: &GenerationResponse) -> String {
    let mut details = Vec::new();

    details.push(format!("candidates={}", response.candidates.len()));

    if let Some(feedback) = &response.prompt_feedback
        && let Some(reason) = &feedback.block_reason
    {
        details.push(format!("prompt_blocked={reason:?}"));
    }

    let finish_reasons = response
        .candidates
        .iter()
        .filter_map(|candidate| candidate.finish_reason.as_ref())
        .map(|reason| format!("{reason:?}"))
        .collect::<Vec<_>>();
    if !finish_reasons.is_empty() {
        details.push(format!("finish_reasons=[{}]", finish_reasons.join(", ")));
    }

    // Count parts by type for debugging
    let mut text_count = 0;
    let mut thought_count = 0;
    let mut other_count = 0;
    for candidate in &response.candidates {
        if let Some(parts) = &candidate.content.parts {
            for part in parts {
                match part {
                    Part::Text { thought, .. } => {
                        if thought.unwrap_or(false) {
                            thought_count += 1;
                        } else {
                            text_count += 1;
                        }
                    }
                    _ => other_count += 1,
                }
            }
        }
    }
    details.push(format!(
        "parts={{text={text_count}, thought={thought_count}, other={other_count}}}"
    ));

    details.join(", ")
}

fn response_hit_max_tokens(response: &GenerationResponse) -> bool {
    response.candidates.iter().any(|candidate| {
        matches!(
            candidate.finish_reason,
            Some(gemini_rust::FinishReason::MaxTokens)
        )
    })
}
