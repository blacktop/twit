use anyhow::{Context, Result};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::responses::{
        CreateResponseArgs, ImageDetail, InputContent, InputImageArgs, InputMessageArgs, InputRole,
        InputTextContent, OutputItem, OutputMessageContent, Response, ResponseTextParam, Status,
        SummaryPart, TextResponseFormatConfiguration, Verbosity,
    },
};

use crate::ai::{
    ImageSummaryInput, SummaryInput, build_image_prompt, build_user_prompt,
    effective_max_output_tokens, effective_system_prompt,
};
use crate::config::AiConfig;
use std::env;

fn build_client(http: &reqwest::Client, config: &AiConfig) -> Result<Client<OpenAIConfig>> {
    let api_key = config
        .api_key
        .as_deref()
        .map(str::to_string)
        .or_else(|| env::var("OPENAI_API_KEY").ok())
        .context("OpenAI API key is missing (set ai.api_key or OPENAI_API_KEY)")?;

    let mut openai_config = OpenAIConfig::new().with_api_key(api_key);
    if let Some(base_url) = config.api_base.as_deref() {
        openai_config = openai_config.with_api_base(base_url);
    }

    Ok(Client::with_config(openai_config).with_http_client(http.clone()))
}

pub async fn summarize(
    http: &reqwest::Client,
    config: &AiConfig,
    input: &SummaryInput,
    truncated: bool,
) -> Result<String> {
    let client = build_client(http, config)?;

    let system_prompt = effective_system_prompt(config);
    let user_prompt = build_user_prompt(input, config, truncated);
    let configured_max_output_tokens = config.max_output_tokens;
    let max_output_tokens = effective_max_output_tokens(config);
    let text_config = ResponseTextParam {
        format: TextResponseFormatConfiguration::Text,
        verbosity: Some(Verbosity::Low),
    };

    let mut attempt_max = max_output_tokens;
    let allow_expand = configured_max_output_tokens == 0;
    let mut attempts = 0;
    let response = loop {
        let response = request_response(
            &client,
            config,
            &system_prompt,
            &user_prompt,
            attempt_max,
            &text_config,
        )
        .await?;

        if response.status == Status::Incomplete {
            let reason = response
                .incomplete_details
                .as_ref()
                .map(|details| details.reason.as_str())
                .unwrap_or("");
            if reason == "max_output_tokens" && attempts < 2 && allow_expand {
                attempts += 1;
                attempt_max = attempt_max.saturating_mul(2).min(8192);
                continue;
            }
        }

        break response;
    };

    if response.status != Status::Completed {
        let detail = response_diagnostics(&response);
        anyhow::bail!("OpenAI response status {:?} ({})", response.status, detail);
    }

    let summary = extract_response_text(&response).ok_or_else(|| {
        anyhow::anyhow!(
            "OpenAI response missing output text ({})",
            response_diagnostics(&response)
        )
    })?;

    Ok(summary)
}

pub async fn summarize_images(
    http: &reqwest::Client,
    config: &AiConfig,
    input: &ImageSummaryInput,
    truncated: bool,
) -> Result<String> {
    let client = build_client(http, config)?;

    let system_prompt = effective_system_prompt(config);
    let prompt = build_image_prompt(
        input.tweet_text.as_deref(),
        config,
        input.image_urls.len(),
        truncated,
    );
    let configured_max_output_tokens = config.max_output_tokens;
    let max_output_tokens = effective_max_output_tokens(config);

    let mut contents = Vec::new();
    contents.push(InputContent::InputText(InputTextContent { text: prompt }));
    for url in input.image_urls.iter().filter(|url| !url.trim().is_empty()) {
        let image = InputImageArgs::default()
            .image_url(url.to_string())
            .detail(ImageDetail::Auto)
            .build()
            .context("Failed to build OpenAI image input")?;
        contents.push(InputContent::InputImage(image));
    }

    let message = InputMessageArgs::default()
        .role(InputRole::User)
        .content(contents)
        .build()
        .context("Failed to build OpenAI image message")?;

    let text_config = ResponseTextParam {
        format: TextResponseFormatConfiguration::Text,
        verbosity: Some(Verbosity::Low),
    };

    let mut attempt_max = max_output_tokens;
    let allow_expand = configured_max_output_tokens == 0;
    let mut attempts = 0;
    let response = loop {
        let response = request_response_with_message(
            &client,
            config,
            &system_prompt,
            message.clone(),
            attempt_max,
            &text_config,
        )
        .await?;

        if response.status == Status::Incomplete {
            let reason = response
                .incomplete_details
                .as_ref()
                .map(|details| details.reason.as_str())
                .unwrap_or("");
            if reason == "max_output_tokens" && attempts < 2 && allow_expand {
                attempts += 1;
                attempt_max = attempt_max.saturating_mul(2).min(8192);
                continue;
            }
        }

        break response;
    };

    if response.status != Status::Completed {
        let detail = response_diagnostics(&response);
        anyhow::bail!("OpenAI response status {:?} ({})", response.status, detail);
    }

    let summary = extract_response_text(&response).ok_or_else(|| {
        anyhow::anyhow!(
            "OpenAI response missing output text ({})",
            response_diagnostics(&response)
        )
    })?;

    Ok(summary)
}

async fn request_response(
    client: &Client<OpenAIConfig>,
    config: &AiConfig,
    system_prompt: &str,
    user_prompt: &str,
    max_output_tokens: u32,
    text_config: &ResponseTextParam,
) -> Result<Response> {
    let request = CreateResponseArgs::default()
        .model(&config.model)
        .instructions(system_prompt.to_string())
        .input(user_prompt.to_string())
        .max_output_tokens(max_output_tokens)
        .text(text_config.clone())
        .build()
        .context("Failed to build OpenAI response request")?;

    client
        .responses()
        .create(request)
        .await
        .context("OpenAI request failed")
}

async fn request_response_with_message(
    client: &Client<OpenAIConfig>,
    config: &AiConfig,
    system_prompt: &str,
    message: async_openai::types::responses::InputMessage,
    max_output_tokens: u32,
    text_config: &ResponseTextParam,
) -> Result<Response> {
    let request = CreateResponseArgs::default()
        .model(&config.model)
        .instructions(system_prompt.to_string())
        .input(message)
        .max_output_tokens(max_output_tokens)
        .text(text_config.clone())
        .build()
        .context("Failed to build OpenAI image response request")?;

    client
        .responses()
        .create(request)
        .await
        .context("OpenAI request failed")
}

fn extract_response_text(response: &async_openai::types::responses::Response) -> Option<String> {
    let output_text = response.output_text().map(|text| text.trim().to_string());
    if let Some(text) = output_text.as_deref()
        && !text.is_empty()
    {
        return Some(text.to_string());
    }

    let mut parts = Vec::new();
    for item in &response.output {
        match item {
            OutputItem::Message(message) => {
                for content in &message.content {
                    match content {
                        OutputMessageContent::OutputText(text) => {
                            let trimmed = text.text.trim();
                            if !trimmed.is_empty() {
                                parts.push(trimmed.to_string());
                            }
                        }
                        OutputMessageContent::Refusal(refusal) => {
                            let trimmed = refusal.refusal.trim();
                            if !trimmed.is_empty() {
                                parts.push(trimmed.to_string());
                            }
                        }
                    }
                }
            }
            OutputItem::Reasoning(reasoning) => {
                for summary in &reasoning.summary {
                    let SummaryPart::SummaryText(text) = summary;
                    let trimmed = text.text.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn response_diagnostics(response: &async_openai::types::responses::Response) -> String {
    let error = response
        .error
        .as_ref()
        .map(|err| format!("{} ({})", err.message, err.code))
        .unwrap_or_else(|| "none".to_string());
    let incomplete = response
        .incomplete_details
        .as_ref()
        .map(|details| details.reason.as_str())
        .unwrap_or("none");
    let output_kinds = response
        .output
        .iter()
        .map(output_item_kind)
        .collect::<Vec<_>>();
    let output_summary = if output_kinds.is_empty() {
        "output_items=0".to_string()
    } else {
        format!(
            "output_items={} ({})",
            output_kinds.len(),
            output_kinds.join(", ")
        )
    };
    let usage = response.usage.as_ref().map(|usage| {
        format!(
            "usage input={}, output={}, reasoning={}, total={}",
            usage.input_tokens,
            usage.output_tokens,
            usage.output_tokens_details.reasoning_tokens,
            usage.total_tokens
        )
    });

    format!(
        "error={}, incomplete={}, {}, {}",
        error,
        incomplete,
        output_summary,
        usage.unwrap_or_else(|| "usage=none".to_string())
    )
}

fn output_item_kind(item: &OutputItem) -> &'static str {
    match item {
        OutputItem::Message(_) => "message",
        OutputItem::Reasoning(_) => "reasoning",
        OutputItem::FunctionCall(_) => "function_call",
        OutputItem::FileSearchCall(_) => "file_search_call",
        OutputItem::WebSearchCall(_) => "web_search_call",
        OutputItem::ComputerCall(_) => "computer_call",
        OutputItem::Compaction(_) => "compaction",
        OutputItem::ImageGenerationCall(_) => "image_generation_call",
        OutputItem::CodeInterpreterCall(_) => "code_interpreter_call",
        OutputItem::LocalShellCall(_) => "local_shell_call",
        OutputItem::ShellCall(_) => "shell_call",
        OutputItem::ShellCallOutput(_) => "shell_call_output",
        OutputItem::ApplyPatchCall(_) => "apply_patch_call",
        OutputItem::ApplyPatchCallOutput(_) => "apply_patch_call_output",
        OutputItem::McpCall(_) => "mcp_call",
        OutputItem::McpListTools(_) => "mcp_list_tools",
        OutputItem::McpApprovalRequest(_) => "mcp_approval_request",
        OutputItem::CustomToolCall(_) => "custom_tool_call",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::SummaryInput;
    use crate::config::{AiConfig, AiProvider, SummaryLength};
    use async_openai::types::responses::Response;
    use std::env;

    #[test]
    fn extracts_output_text_from_message() {
        let json = r#"
        {
          "id": "resp_1",
          "object": "response",
          "created_at": 0,
          "model": "gpt-4o-mini",
          "status": "completed",
          "output": [
            {
              "type": "message",
              "id": "msg_1",
              "role": "assistant",
              "status": "completed",
              "content": [
                { "type": "output_text", "text": "Hello world", "annotations": [] }
              ]
            }
          ]
        }
        "#;

        let response: Response = serde_json::from_str(json).unwrap();
        let extracted = extract_response_text(&response).unwrap();
        assert_eq!(extracted, "Hello world");
    }

    #[test]
    fn extracts_reasoning_summary_when_no_output_text() {
        let json = r#"
        {
          "id": "resp_2",
          "object": "response",
          "created_at": 0,
          "model": "gpt-4o-mini",
          "status": "completed",
          "output": [
            {
              "type": "reasoning",
              "id": "reason_1",
              "summary": [
                { "type": "summary_text", "text": "Reasoning summary." }
              ]
            }
          ]
        }
        "#;

        let response: Response = serde_json::from_str(json).unwrap();
        let extracted = extract_response_text(&response).unwrap();
        assert_eq!(extracted, "Reasoning summary.");
    }

    #[tokio::test]
    async fn integration_openai_summary_env_gated() {
        let enabled = env::var("TWIT_OPENAI_TEST").ok();
        if enabled.as_deref() != Some("1") {
            return;
        }
        if env::var("OPENAI_API_KEY").is_err() {
            return;
        }

        let config = AiConfig {
            enabled: true,
            provider: AiProvider::OpenAI,
            model: env::var("TWIT_OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string()),
            summary_length: SummaryLength::Short,
            system_prompt: "Summarize in one short paragraph.".to_string(),
            max_output_tokens: 1024,
            api_base: env::var("TWIT_OPENAI_BASE").ok(),
            ..Default::default()
        };

        let input = SummaryInput {
            text: "OpenAI provides an API that lets developers generate text, analyze content, and build AI-powered workflows."
                .to_string(),
            source_url: Some("https://example.com".to_string()),
            title: Some("Example Article".to_string()),
        };

        let http = reqwest::Client::new();
        let summary = summarize(&http, &config, &input, false).await.unwrap();
        assert!(!summary.trim().is_empty());
    }
}
