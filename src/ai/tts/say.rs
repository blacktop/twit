use anyhow::{Context, Result};
use std::process::{Child, Command, Stdio};

use crate::config::TtsConfig;

pub fn speak(config: &TtsConfig, text: &str) -> Result<Child> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("macOS say is only available on macOS");
    }

    // Sanitize text - say interprets <> as embedded speech commands
    let sanitized = text.replace(['<', '>'], "");

    let mut command = Command::new("say");
    // Suppress stderr - say outputs system diagnostic messages that pollute TUI
    command.stderr(Stdio::null());

    if let Some(voice) = &config.voice
        && !voice.trim().is_empty()
    {
        command.arg("-v").arg(voice);
    }
    if let Some(rate) = config.rate_wpm {
        command.arg("-r").arg(rate.to_string());
    }
    command.arg(&sanitized);

    command.spawn().context("Failed to launch macOS say")
}
