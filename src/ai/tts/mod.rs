use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{TtsConfig, TtsProvider};
use crate::logging;

pub(crate) mod catalog;
mod elevenlabs;
mod google;
mod openai;
mod say;

pub struct TtsClient {
    config: TtsConfig,
    /// Running `say` process (if any)
    say_child: Option<Child>,
    /// Cancellation flag for file-based playback threads
    cancel_flag: Arc<AtomicBool>,
}

impl TtsClient {
    pub fn new(config: TtsConfig) -> Self {
        Self {
            config,
            say_child: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Stop any currently running TTS playback
    pub fn stop(&mut self) {
        // Kill say process if running
        if let Some(ref mut child) = self.say_child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.say_child = None;

        // Signal file playback threads to stop
        self.cancel_flag.store(true, Ordering::SeqCst);
        // Create new flag for future playback
        self.cancel_flag = Arc::new(AtomicBool::new(false));
    }

    pub async fn speak(&mut self, text: &str) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Stop any currently playing TTS before starting new
        self.stop();

        match self.config.provider {
            TtsProvider::Say => {
                let child = say::speak(&self.config, text)?;
                self.say_child = Some(child);
                Ok(())
            }
            TtsProvider::OpenAI => {
                openai::speak(&self.config, text, self.cancel_flag.clone()).await
            }
            TtsProvider::Google => {
                google::speak(&self.config, text, self.cancel_flag.clone()).await
            }
            TtsProvider::ElevenLabs => {
                elevenlabs::speak(&self.config, text, self.cancel_flag.clone()).await
            }
        }
    }
}

pub(crate) fn temp_audio_path(extension: &str) -> Result<PathBuf> {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Failed to read system time")?;
    let filename = format!("twit-tts-{}.{}", since_epoch.as_nanos(), extension);
    Ok(env::temp_dir().join(filename))
}

pub(crate) fn play_audio(path: &Path, cancel: Arc<AtomicBool>) -> Result<()> {
    let path = path.to_path_buf();
    thread::spawn(move || {
        // Check cancellation before starting
        if cancel.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&path);
            return;
        }

        if let Err(err) = play_audio_with_rodio(&path, &cancel) {
            logging::log_error("tts", &format!("rodio playback failed: {}", err));
            #[cfg(target_os = "macos")]
            if !cancel.load(Ordering::SeqCst)
                && let Err(err) = play_audio_with_afplay(&path)
            {
                logging::log_error("tts", &format!("afplay failed: {}", err));
            }
            #[cfg(not(target_os = "macos"))]
            if !cancel.load(Ordering::SeqCst)
                && let Err(err) = open::that(&path)
            {
                logging::log_error("tts", &format!("open failed: {}", err));
            }
        }
        let _ = fs::remove_file(path);
    });
    Ok(())
}

fn play_audio_with_rodio(path: &Path, cancel: &AtomicBool) -> Result<()> {
    let file = fs::File::open(path).context("Failed to open audio file")?;
    let reader = BufReader::new(file);
    let stream_handle =
        rodio::OutputStreamBuilder::open_default_stream().context("Failed to open audio output")?;
    let sink = rodio::Sink::connect_new(stream_handle.mixer());
    let source = rodio::Decoder::new(reader).context("Failed to decode audio stream")?;
    sink.append(source);

    // Poll for cancellation instead of blocking
    while !sink.empty() {
        if cancel.load(Ordering::SeqCst) {
            sink.stop();
            return Ok(());
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn play_audio_with_afplay(path: &Path) -> Result<()> {
    let status = Command::new("afplay")
        .arg(path)
        .status()
        .context("Failed to run afplay")?;
    if !status.success() {
        anyhow::bail!("afplay exited with status {}", status);
    }
    Ok(())
}

pub(crate) fn audio_extension_from_mime(mime_type: &str) -> &'static str {
    match mime_type.split(';').next().unwrap_or(mime_type).trim() {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/aac" => "aac",
        "audio/flac" => "flac",
        "audio/pcm" => "pcm",
        _ => "bin",
    }
}

pub(crate) fn pcm_to_wav(
    pcm: &[u8],
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
) -> Vec<u8> {
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * (bits_per_sample / 8);
    let data_len = pcm.len() as u32;
    let riff_chunk_size = 36 + data_len;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_chunk_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}
