# twit

A terminal UI client for Twitter/X.

## Requirements and constraints

- 2FA must be disabled for the account you use with this tool.

> [!WARNING]  
> This may violate Twitter/X ToS. Use a fresh burner account and do NOT reuse your primary account.

## Getting Started

### Install

Homebrew (macOS):

```sh
brew install --cask blacktop/tap/twit
```

GitHub Releases:

1. Download the latest release from [GitHub Releases](https://github.com/blacktop/twit/releases).
2. Extract and move the binary into your `PATH`:

```sh
chmod +x twit
mv twit /usr/local/bin/
```

### Build from source

```sh
git clone https://github.com/blacktop/twit.git
cd twit
cargo build --release
./target/release/twit
```


## Auth cookies (auth_token + ct0)

You must extract cookies from an active X session in your browser.

Firefox:
1. Log in to X.
2. Open DevTools → Storage → Cookies → `https://x.com`.
3. Copy the `auth_token` and `ct0` cookie values.

Chrome:
1. Log in to X.
2. Open DevTools → Application → Cookies → `https://x.com`.
3. Copy the `auth_token` and `ct0` cookie values.

Put these values in `config.yaml` (see `config.yml.example`).

## Power features (AI + TTS)

twit can summarize links and tweets (and optionally images) with multiple AI providers, and read summaries aloud via TTS.

### AI summarization

Supported providers include:
- OpenAI
- Anthropic
- Google (Gemini)
- OpenRouter
- Copilot
- Local (Ollama / LM Studio)
- Apple Foundation Models (macOS only)

Behavior is configured under `ai:` in `config.yaml`:
- `enabled`, `provider`, `model`
- `summary_length` (short/medium/long)
- `system_prompt` (controls tone/style)
- `summarize_links`, `summarize_tweets`, `summarize_images`
- `max_output_tokens` (0 = auto)

API keys can come from `config.yaml` or env vars (see `config.yml.example`).

### Text-to-speech

Supported TTS providers include:
- macOS `say`
- OpenAI
- Google
- ElevenLabs

Behavior is configured under `tts:` in `config.yaml`:
- `enabled`, `provider`, `voice`, `rate_wpm`
- `auto_speak_summaries`

Use the TUI keys `s` to summarize and `v` to speak.

## Configuration

Config file location:
- macOS: `~/Library/Application Support/io.blacktop.twit/config.yaml`
- Linux: `~/.config/twit/config.yaml`
- Windows: `%APPDATA%\\io.blacktop.twit\\config.yaml`

Start from `config.yml.example` and copy it to the location above.

## Debug logging

Logging is disabled by default. To enable debug logging:

```sh
# Via CLI flag (for one session)
twit --debug

# Or set in config.yaml (persistent)
debug: true
```

Logs are written to:
- macOS: `~/Library/Caches/io.blacktop.twit/twit.log`
- Linux: `~/.cache/twit/twit.log`

## License

MIT License - see [LICENSE](LICENSE) for details.
