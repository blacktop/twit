use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose};
use gemini_rust::{
    GeminiBuilder, GenerationConfig, Part, PrebuiltVoiceConfig, SpeechConfig, VoiceConfig,
};
use reqwest::ClientBuilder;
use std::env;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use url::Url;

use crate::ai::tts::{audio_extension_from_mime, pcm_to_wav, play_audio, temp_audio_path};
use crate::config::TtsConfig;

const DEFAULT_GOOGLE_TTS_MODEL: &str = "models/gemini-2.5-flash-preview-tts";
const DEFAULT_GOOGLE_TTS_VOICE: &str = "Kore";
pub const DEFAULT_GOOGLE_TTS_TIMEOUT_SECS: u64 = 60;
const DEFAULT_GOOGLE_PCM_SAMPLE_RATE: u32 = 24_000;
const DEFAULT_GOOGLE_PCM_CHANNELS: u16 = 1;
const DEFAULT_GOOGLE_PCM_BITS_PER_SAMPLE: u16 = 16;

pub async fn speak(config: &TtsConfig, text: &str, cancel: Arc<AtomicBool>) -> Result<()> {
    let api_key = google_api_key(config).ok_or_else(|| {
        anyhow::anyhow!("Gemini API key missing (set tts.api_key or GEMINI_API_KEY)")
    })?;
    let input = text.trim();
    if input.is_empty() {
        anyhow::bail!("No text provided for Gemini TTS");
    }

    let model = config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_GOOGLE_TTS_MODEL);
    let voice = config
        .voice
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_GOOGLE_TTS_VOICE);

    let mut builder = GeminiBuilder::new(api_key)
        .with_model(parse_google_model(model))
        .with_http_client(
            ClientBuilder::new()
                .user_agent("twit-ai/0.1")
                .timeout(Duration::from_secs(DEFAULT_GOOGLE_TTS_TIMEOUT_SECS))
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .https_only(true),
        );

    if let Some(base_url) = config.api_base.as_deref() {
        let url = Url::parse(base_url.trim())
            .with_context(|| format!("Invalid Gemini base URL '{}'", base_url))?;
        builder = builder.with_base_url(url);
    }

    let client = builder.build().context("Failed to build Gemini client")?;

    let generation_config = GenerationConfig {
        response_modalities: Some(vec!["AUDIO".to_string()]),
        speech_config: Some(SpeechConfig {
            voice_config: Some(VoiceConfig {
                prebuilt_voice_config: Some(PrebuiltVoiceConfig {
                    voice_name: voice.to_string(),
                }),
            }),
            multi_speaker_voice_config: None,
        }),
        ..Default::default()
    };

    let response = client
        .generate_content()
        .with_user_message(input)
        .with_generation_config(generation_config)
        .execute()
        .await
        .context("Gemini TTS request failed")?;

    let (audio_bytes, mime_type) = extract_google_audio(&response)?;
    let (payload, extension) = match mime_type.as_str() {
        "audio/pcm" => (
            pcm_to_wav(
                &audio_bytes,
                DEFAULT_GOOGLE_PCM_SAMPLE_RATE,
                DEFAULT_GOOGLE_PCM_CHANNELS,
                DEFAULT_GOOGLE_PCM_BITS_PER_SAMPLE,
            ),
            "wav",
        ),
        _ => (audio_bytes, audio_extension_from_mime(&mime_type)),
    };
    let path = temp_audio_path(extension)?;
    std::fs::write(&path, &payload).context("Failed to write Gemini TTS audio file")?;

    play_audio(&path, cancel)?;

    Ok(())
}

fn google_api_key(config: &TtsConfig) -> Option<String> {
    config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            env::var("GEMINI_API_KEY")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
}

fn parse_google_model(value: &str) -> gemini_rust::Model {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return gemini_rust::Model::Custom(DEFAULT_GOOGLE_TTS_MODEL.to_string());
    }
    if trimmed.starts_with("models/") {
        return gemini_rust::Model::Custom(trimmed.to_string());
    }
    gemini_rust::Model::Custom(format!("models/{}", trimmed))
}

fn extract_google_audio(response: &gemini_rust::GenerationResponse) -> Result<(Vec<u8>, String)> {
    for candidate in &response.candidates {
        if let Some(parts) = &candidate.content.parts {
            for part in parts {
                if let Part::InlineData { inline_data, .. } = part
                    && inline_data.mime_type.starts_with("audio/")
                {
                    let audio_bytes = general_purpose::STANDARD
                        .decode(&inline_data.data)
                        .context("Failed to decode Gemini audio payload")?;
                    return Ok((audio_bytes, inline_data.mime_type.clone()));
                }
            }
        }
    }

    anyhow::bail!("Gemini response missing audio data")
}
