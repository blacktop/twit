use anyhow::{Context, Result};
use std::env;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::ai::tts::{audio_extension_from_mime, play_audio, temp_audio_path};
use crate::config::TtsConfig;

const DEFAULT_ELEVENLABS_TTS_MODEL: &str = "eleven_multilingual_v2";
const DEFAULT_ELEVENLABS_TTS_TIMEOUT_SECS: u64 = 60;
const DEFAULT_ELEVENLABS_BASE_URL: &str = "https://api.elevenlabs.io/v1";
const DEFAULT_ELEVENLABS_VOICE_ID: &str = "1SM7GgM6IMuvQlz2BwM3";

pub async fn speak(config: &TtsConfig, text: &str, cancel: Arc<AtomicBool>) -> Result<()> {
    let api_key = elevenlabs_api_key(config).ok_or_else(|| {
        anyhow::anyhow!("ElevenLabs API key missing (set tts.api_key or ELEVENLABS_API_KEY)")
    })?;
    let input = text.trim();
    if input.is_empty() {
        anyhow::bail!("No text provided for ElevenLabs TTS");
    }

    let voice_id = config
        .voice
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_ELEVENLABS_VOICE_ID);

    let model = config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_ELEVENLABS_TTS_MODEL);

    let base_url = config
        .api_base
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_ELEVENLABS_BASE_URL)
        .trim_end_matches('/');

    let url = format!("{}/text-to-speech/{}", base_url, voice_id);
    let request_body = serde_json::json!({
        "text": input,
        "model_id": model,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_ELEVENLABS_TTS_TIMEOUT_SECS))
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .https_only(true)
        .build()
        .context("Failed to build ElevenLabs HTTP client")?;

    let response = client
        .post(url)
        .header("xi-api-key", api_key)
        .header(reqwest::header::ACCEPT, "audio/mpeg")
        .json(&request_body)
        .send()
        .await
        .context("ElevenLabs TTS request failed")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED && body.contains("quota_exceeded") {
            anyhow::bail!(
                "ElevenLabs quota exceeded. Reduce summary length or switch TTS providers."
            );
        }
        anyhow::bail!("ElevenLabs TTS request failed with {}: {}", status, body);
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("audio/mpeg")
        .to_string();
    let audio = response
        .bytes()
        .await
        .context("Failed to read ElevenLabs audio")?;
    let extension = audio_extension_from_mime(&content_type);
    let path = temp_audio_path(extension)?;
    std::fs::write(&path, &audio).context("Failed to write ElevenLabs audio file")?;

    play_audio(&path, cancel)?;

    Ok(())
}

fn elevenlabs_api_key(config: &TtsConfig) -> Option<String> {
    config
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            env::var("ELEVENLABS_API_KEY")
                .ok()
                .or_else(|| env::var("XI_API_KEY").ok())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
}
