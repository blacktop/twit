use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::ai::{
    ImageSummaryInput, SummaryInput, build_user_prompt, effective_max_output_tokens,
    effective_system_prompt,
};
use crate::config::AiConfig;
use crate::logging;

#[cfg(target_os = "macos")]
use fm_rs::{
    ContextLimit, GenerationOptions, ModelAvailability, Session, SystemLanguageModel,
    context_usage_from_transcript,
};

pub async fn summarize(config: &AiConfig, input: &SummaryInput, truncated: bool) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let config = config.clone();
        let input = input.clone();
        let summary =
            tokio::task::spawn_blocking(move || summarize_blocking(&config, &input, truncated))
                .await
                .context("FoundationModels task join failed")??;
        Ok(summary)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (config, input, truncated);
        anyhow::bail!("FoundationModels provider is only available on macOS")
    }
}

pub async fn summarize_images(
    _config: &AiConfig,
    _input: &ImageSummaryInput,
    _truncated: bool,
) -> Result<String> {
    anyhow::bail!("FoundationModels does not support image summaries yet");
}

pub async fn summarize_streaming(
    config: &AiConfig,
    input: &SummaryInput,
    truncated: bool,
) -> Result<mpsc::Receiver<String>> {
    #[cfg(target_os = "macos")]
    {
        let model =
            SystemLanguageModel::new().context("Failed to create FoundationModels system model")?;

        match model.availability() {
            ModelAvailability::Available => {}
            ModelAvailability::DeviceNotEligible => {
                anyhow::bail!(
                    "FoundationModels unavailable: device not eligible for Apple Intelligence"
                );
            }
            ModelAvailability::AppleIntelligenceNotEnabled => {
                anyhow::bail!("FoundationModels unavailable: Apple Intelligence is not enabled");
            }
            ModelAvailability::ModelNotReady => {
                anyhow::bail!("FoundationModels unavailable: model not ready");
            }
            ModelAvailability::Unknown => {
                anyhow::bail!("FoundationModels unavailable: unknown availability state");
            }
        }

        let instructions = effective_system_prompt(config);
        let prompt = build_user_prompt(input, config, truncated);
        let max_tokens = foundation_response_tokens(config);
        let max_tokens_usize = usize::try_from(max_tokens).unwrap_or(0);
        let mut options_builder = GenerationOptions::builder();
        if max_tokens > 0 {
            options_builder = options_builder.max_response_tokens(max_tokens);
        }
        let options = options_builder.build();

        let (tx, rx) = mpsc::channel(32);

        tokio::task::spawn_blocking(move || {
            let sender = tx;
            let mut last_chunk = String::new();
            let session = if instructions.is_empty() {
                match Session::new(&model) {
                    Ok(session) => session,
                    Err(err) => {
                        logging::log_error(
                            "summary",
                            &format!("FoundationModels session init failed: {}", err),
                        );
                        return;
                    }
                }
            } else {
                match Session::with_instructions(&model, &instructions) {
                    Ok(session) => session,
                    Err(err) => {
                        logging::log_error(
                            "summary",
                            &format!(
                                "FoundationModels session init (instructions) failed: {}",
                                err
                            ),
                        );
                        return;
                    }
                }
            };

            let prompt =
                match prepare_prompt_with_context(&model, &instructions, &prompt, max_tokens_usize)
                {
                    Ok(prompt) => prompt,
                    Err(err) => {
                        logging::log_error(
                            "summary",
                            &format!("FoundationModels prompt preparation failed: {}", err),
                        );
                        return;
                    }
                };

            let result = session.stream_response(&prompt, &options, move |chunk| {
                if chunk.is_empty() {
                    return;
                }

                let delta = if chunk.starts_with(&last_chunk) {
                    &chunk[last_chunk.len()..]
                } else if last_chunk.starts_with(chunk) {
                    ""
                } else {
                    chunk
                };

                if !delta.is_empty() {
                    let _ = sender.blocking_send(delta.to_string());
                }

                last_chunk.clear();
                last_chunk.push_str(chunk);
            });

            if let Err(err) = result {
                logging::log_error(
                    "summary",
                    &format!("FoundationModels streaming failed: {}", err),
                );
            }
        });

        Ok(rx)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (config, input, truncated);
        anyhow::bail!("FoundationModels provider is only available on macOS")
    }
}

#[cfg(target_os = "macos")]
fn summarize_blocking(config: &AiConfig, input: &SummaryInput, truncated: bool) -> Result<String> {
    let model =
        SystemLanguageModel::new().context("Failed to create FoundationModels system model")?;

    match model.availability() {
        ModelAvailability::Available => {}
        ModelAvailability::DeviceNotEligible => {
            anyhow::bail!(
                "FoundationModels unavailable: device not eligible for Apple Intelligence"
            );
        }
        ModelAvailability::AppleIntelligenceNotEnabled => {
            anyhow::bail!("FoundationModels unavailable: Apple Intelligence is not enabled");
        }
        ModelAvailability::ModelNotReady => {
            anyhow::bail!("FoundationModels unavailable: model not ready");
        }
        ModelAvailability::Unknown => {
            anyhow::bail!("FoundationModels unavailable: unknown availability state");
        }
    }

    let instructions = effective_system_prompt(config);
    let session = if instructions.is_empty() {
        Session::new(&model).context("Failed to create FoundationModels session")?
    } else {
        Session::with_instructions(&model, &instructions)
            .context("Failed to create FoundationModels session with instructions")?
    };

    let prompt = build_user_prompt(input, config, truncated);
    let max_tokens = foundation_response_tokens(config);
    let max_tokens_usize = usize::try_from(max_tokens).unwrap_or(0);
    let mut options_builder = GenerationOptions::builder();
    if max_tokens > 0 {
        options_builder = options_builder.max_response_tokens(max_tokens);
    }
    let options = options_builder.build();

    let prompt = prepare_prompt_with_context(&model, &instructions, &prompt, max_tokens_usize)?;

    let response = session
        .respond(&prompt, &options)
        .context("FoundationModels response failed")?;

    Ok(response.into_content())
}

#[cfg(target_os = "macos")]
fn prepare_prompt_with_context(
    _model: &SystemLanguageModel,
    instructions: &str,
    prompt: &str,
    max_response_tokens: usize,
) -> Result<String> {
    let reserved_tokens = if max_response_tokens > 0 {
        max_response_tokens
    } else {
        256
    };
    let limit = ContextLimit::default_on_device().with_reserved_response_tokens(reserved_tokens);

    // Calculate tokens used by instructions alone so we can subtract from budget
    let instructions_tokens = if instructions.trim().is_empty() {
        0
    } else {
        let instructions_json = build_transcript_json(instructions, "")?;
        let instructions_usage = context_usage_from_transcript(&instructions_json, &limit)
            .context("Failed to estimate instructions token usage")?;
        instructions_usage.estimated_tokens
    };

    let transcript_json = build_transcript_json(instructions, prompt)?;
    let usage = context_usage_from_transcript(&transcript_json, &limit)
        .context("Failed to estimate FoundationModels context usage")?;

    // Calculate safe budget for user prompt (excluding instructions)
    // FoundationModels has ~500 token overhead beyond what we estimate
    let model_overhead = 512;
    let prompt_budget = usage
        .available_tokens
        .saturating_sub(instructions_tokens)
        .saturating_sub(model_overhead);
    let safe_tokens = (prompt_budget * 6) / 10; // 60% margin for safety

    // Log current usage for debugging
    logging::log_info(
        "summary",
        &format!(
            "FoundationModels context: {} tokens estimated, {} available, {}% util",
            usage.estimated_tokens,
            usage.available_tokens,
            (usage.utilization * 100.0) as u32
        ),
    );

    // Trigger truncation at 70% to leave headroom for model overhead
    if !usage.over_limit && usage.utilization <= 0.70 {
        log_prompt_preview("original", prompt);
        return Ok(prompt.to_string());
    }

    logging::log_info(
        "summary",
        &format!(
            "FoundationModels context high ({}% utilization) - truncating to {} tokens",
            (usage.utilization * 100.0) as u32,
            safe_tokens
        ),
    );

    // Simple truncation - compaction adds complexity and can fail
    let truncated = truncate_prompt(prompt, safe_tokens, limit.chars_per_token)?;
    log_prompt_preview("truncated", &truncated);
    Ok(truncated)
}

#[cfg(target_os = "macos")]
fn build_transcript_json(instructions: &str, prompt: &str) -> Result<String> {
    let mut items = Vec::new();
    if !instructions.trim().is_empty() {
        items.push(serde_json::json!({
            "role": "system",
            "content": instructions.trim(),
        }));
    }
    items.push(serde_json::json!({
        "role": "user",
        "content": prompt.trim(),
    }));
    serde_json::to_string(&items).context("Failed to build FoundationModels transcript JSON")
}

#[cfg(target_os = "macos")]
fn foundation_response_tokens(config: &AiConfig) -> u32 {
    let configured = effective_max_output_tokens(config);
    if configured == 0 {
        256
    } else {
        configured.min(256)
    }
}

#[cfg(target_os = "macos")]
fn log_prompt_preview(label: &str, prompt: &str) {
    let preview_len = 500usize;
    let preview: String = prompt.chars().take(preview_len).collect();
    let suffix = if prompt.chars().count() > preview_len {
        "…"
    } else {
        ""
    };
    logging::log_info(
        "summary",
        &format!(
            "FoundationModels prompt {} (chars={}): {}{}",
            label,
            prompt.chars().count(),
            preview,
            suffix
        ),
    );
}

#[cfg(target_os = "macos")]
fn truncate_prompt(prompt: &str, max_tokens: usize, chars_per_token: usize) -> Result<String> {
    let max_chars = max_tokens.saturating_mul(chars_per_token.max(1)).max(1);
    if max_chars == 0 {
        anyhow::bail!("FoundationModels prompt exceeds context window");
    }
    Ok(prompt.chars().take(max_chars).collect())
}
