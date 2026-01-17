const OPENAI_TTS_MODELS: &[&str] = &[
    "gpt-4o-mini-tts-2025-12-15",
    "gpt-4o-mini-tts",
    "gpt-4o-audio-preview",
    "tts-1",
    "tts-1-hd",
];

const OPENAI_TTS_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse",
];

const GOOGLE_TTS_MODELS: &[&str] = &[
    "gemini-2.5-flash-preview-tts",
    "gemini-2.5-pro-preview-tts",
    "gemini-2.5-flash-lite-preview-tts",
];

const GOOGLE_TTS_VOICES: &[&str] = &[
    "Achernar",
    "Achird",
    "Algenib",
    "Algieba",
    "Alnilam",
    "Aoede",
    "Autonoe",
    "Callirrhoe",
    "Charon",
    "Despina",
    "Enceladus",
    "Erinome",
    "Fenrir",
    "Gacrux",
    "Iapetus",
    "Kore",
    "Laomedeia",
    "Leda",
    "Orus",
    "Puck",
    "Pulcherrima",
    "Rasalgethi",
    "Sadachbia",
    "Sadaltager",
    "Schedar",
    "Sulafat",
    "Umbriel",
    "Vindemiatrix",
    "Zephyr",
    "Zubenelgenubi",
];

#[allow(dead_code)]
const SAY_VOICES: &[&str] = &[
    "Isha (Premium)",
    "Serena (Premium)",
    "Zoe (Premium)",
    "Evan (Enhanced)",
];

pub fn is_openai_model(value: &str) -> bool {
    OPENAI_TTS_MODELS.contains(&value)
}

pub fn is_openai_voice(value: &str) -> bool {
    OPENAI_TTS_VOICES.contains(&value)
}

pub fn is_google_model(value: &str) -> bool {
    GOOGLE_TTS_MODELS.contains(&value)
}

pub fn is_google_voice(value: &str) -> bool {
    GOOGLE_TTS_VOICES.contains(&value)
}
