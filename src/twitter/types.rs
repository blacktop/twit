use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tweet {
    pub id: String,
    pub text: String,
    pub user: User,
    pub created_at: DateTime<Utc>,
    pub media: Vec<Media>,
    pub retweet_count: u64,
    pub like_count: u64,
    pub reply_count: u64,
    pub is_retweet: bool,
    pub retweeted_by: Option<String>,
    pub is_quote: bool,
    pub quoted_tweet: Option<Box<Tweet>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub screen_name: String,
    pub profile_image_url: String,
    pub verified: bool,
    pub blue_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Media {
    pub url: String,
    pub media_type: MediaType,
    pub width: u32,
    pub height: u32,
    /// Preview/thumbnail URL for videos
    pub preview_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaType {
    Photo,
    Video,
    Gif,
}

impl Tweet {
    /// Get the URL to view this tweet on Twitter/X
    pub fn url(&self) -> String {
        format!("https://x.com/{}/status/{}", self.user.screen_name, self.id)
    }
}

impl User {
    /// Get higher resolution avatar URL (_bigger instead of _normal)
    pub fn avatar_url_bigger(&self) -> String {
        self.profile_image_url.replace("_normal.", "_bigger.")
    }
}

impl Media {
    /// Get smaller image URL for faster loading (Twitter CDN :small suffix)
    pub fn small_url(&self) -> Option<String> {
        (self.media_type == MediaType::Photo).then(|| {
            if self.url.contains("pbs.twimg.com") {
                format!("{}:small", self.url)
            } else {
                self.url.clone()
            }
        })
    }

    /// Get larger image URL for AI analysis (Twitter CDN :large suffix)
    pub fn ai_url(&self) -> Option<String> {
        (self.media_type == MediaType::Photo).then(|| {
            if self.url.contains("pbs.twimg.com") {
                format!("{}:large", self.url)
            } else {
                self.url.clone()
            }
        })
    }
}
