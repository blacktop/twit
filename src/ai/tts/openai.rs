use anyhow::{Context, Result};
use async_openai::Client as OpenAiClient;
use async_openai::config::OpenAIConfig;
use async_openai::types::audio::{
    CreateSpeechRequestArgs, SpeechModel, SpeechResponseFormat, Voice,
};
use std::env;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::ai::tts::{play_audio, temp_audio_path};
use crate::config::TtsConfig;

const DEFAULT_OPENAI_TTS_MODEL: &str = "gpt-4o-mini-tts-2025-12-15";
const DEFAULT_OPENAI_TTS_VOICE: &str = "alloy";
const DEFAULT_OPENAI_TTS_FORMAT: &str = "mp3";

pub async fn speak(config: &TtsConfig, text: &str, cancel: Arc<AtomicBool>) -> Result<()> {
    let api_key = openai_api_key(config).ok_or_else(|| {
        anyhow::anyhow!("OpenAI API key missing (set tts.api_key or OPENAI_API_KEY)")
    })?;
    let input = text.trim();
    if input.is_empty() {
        anyhow::bail!("No text provided for OpenAI TTS");
    }

    let model = config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_OPENAI_TTS_MODEL);
    let voice = config
        .voice
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_OPENAI_TTS_VOICE);
    let base_url = config
        .api_base
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("https://api.openai.com/v1")
        .trim_end_matches('/')
        .to_string();

    let openai_config = OpenAIConfig::new()
        .with_api_key(api_key)
        .with_api_base(base_url);
    let client = OpenAiClient::with_config(openai_config);

    let request = CreateSpeechRequestArgs::default()
        .model(openai_speech_model(model))
        .voice(openai_voice(voice))
        .input(input)
        .response_format(SpeechResponseFormat::Mp3)
        .build()
        .context("Failed to build OpenAI TTS request")?;

    let response = client
        .audio()
        .speech()
        .create(request)
        .await
        .context("OpenAI TTS request failed")?;

    let audio = response.bytes;
    let path = temp_audio_path(DEFAULT_OPENAI_TTS_FORMAT)?;
    std::fs::write(&path, &audio).context("Failed to write OpenAI TTS audio file")?;

    play_audio(&path, cancel)?;

    Ok(())
}

fn openai_api_key(config: &TtsConfig) -> Option<String> {
    config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            env::var("OPENAI_API_KEY")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
}

fn openai_speech_model(value: &str) -> SpeechModel {
    match value.trim() {
        "tts-1" => SpeechModel::Tts1,
        "tts-1-hd" => SpeechModel::Tts1Hd,
        "gpt-4o-mini-tts" => SpeechModel::Gpt4oMiniTts,
        other => SpeechModel::Other(other.to_string()),
    }
}

fn openai_voice(value: &str) -> Voice {
    match value.trim().to_ascii_lowercase().as_str() {
        "alloy" => Voice::Alloy,
        "ash" => Voice::Ash,
        "ballad" => Voice::Ballad,
        "coral" => Voice::Coral,
        "echo" => Voice::Echo,
        "fable" => Voice::Fable,
        "onyx" => Voice::Onyx,
        "nova" => Voice::Nova,
        "sage" => Voice::Sage,
        "shimmer" => Voice::Shimmer,
        "verse" => Voice::Verse,
        other => Voice::Other(other.to_string()),
    }
}
