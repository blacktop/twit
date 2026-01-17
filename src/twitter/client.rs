use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::header::{AUTHORIZATION, COOKIE, HeaderMap, HeaderValue};
use serde_json::Value;
use x_client_transaction::ClientTransaction;

use crate::logging;
use crate::twitter::types::{Media, MediaType, Tweet, User};

/// Extension trait for extracting values from JSON with fallbacks
trait ValueExt {
    fn str_at(&self, key: &str) -> &str;
    fn u64_at(&self, key: &str) -> u64;
    fn bool_at(&self, key: &str) -> bool;
}

impl ValueExt for Value {
    fn str_at(&self, key: &str) -> &str {
        self.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }

    fn u64_at(&self, key: &str) -> u64 {
        self.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
    }

    fn bool_at(&self, key: &str) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }
}

/// Twitter's public web application bearer token.
/// This is NOT a secret - it's the same token used by all browser-based Twitter/X clients.
/// It identifies requests as coming from the official web app and is required for API access.
/// This token is publicly documented and can be found in Twitter's web client JavaScript.
const BEARER_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs=1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

// HomeLatestTimeline = Following (chronological) feed
// HomeTimeline = For You (algorithmic) feed
const HOME_TIMELINE_PATH: &str = "/i/api/graphql/zCOPiWiP4fZS_547OU4yHA/HomeLatestTimeline";
// User lookup by screen name
const USER_BY_SCREEN_NAME_PATH: &str = "/i/api/graphql/xmU6X_CKVnQ5lSrCbAmJsg/UserByScreenName";
// Get user's following list
const FOLLOWING_PATH: &str = "/i/api/graphql/eWTmcJY3EMh-dxIR7CYTKw/Following";
// Follow a user
const CREATE_FRIENDSHIP_PATH: &str = "/i/api/1.1/friendships/create.json";
// Current Chrome on macOS - update periodically to stay current
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn build_timeline_variables(count: usize, cursor: Option<&str>) -> Value {
    let mut vars = serde_json::json!({
        "count": count,
        "includePromotedContent": false,
        "latestControlAvailable": true,
        "requestContext": "launch",
        "withCommunity": true
    });

    if let Some(cursor) = cursor {
        vars["cursor"] = Value::String(cursor.to_string());
    }

    vars
}

fn build_timeline_features() -> Value {
    serde_json::json!({
        "rweb_tipjar_consumption_enabled": true,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "verified_phone_label_enabled": false,
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "responsive_web_graphql_timeline_navigation_enabled": true,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "communities_web_enable_tweet_community_results_fetch": true,
        "c9s_tweet_anatomy_moderator_badge_enabled": true,
        "articles_preview_enabled": true,
        "responsive_web_edit_tweet_api_enabled": true,
        "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
        "view_counts_everywhere_api_enabled": true,
        "longform_notetweets_consumption_enabled": true,
        "responsive_web_twitter_article_tweet_consumption_enabled": true,
        "tweet_awards_web_tipping_enabled": false,
        "creator_subscriptions_quote_tweet_preview_enabled": false,
        "freedom_of_speech_not_reach_fetch_enabled": true,
        "standardized_nudges_misinfo": true,
        "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
        "rweb_video_timestamps_enabled": true,
        "longform_notetweets_rich_text_read_enabled": true,
        "longform_notetweets_inline_media_enabled": true,
        "responsive_web_enhance_cards_enabled": false,
        "blue_business_profile_image_shape_enabled": false,
        "responsive_web_text_conversations_enabled": false,
        "vibe_api_enabled": false,
        "tweetypie_unmention_optimization_enabled": false,
        "interactive_text_enabled": false,
        "responsive_web_grok_annotations_enabled": false,
        "responsive_web_profile_redirect_enabled": false,
        "responsive_web_jetfuel_frame": false,
        "responsive_web_grok_imagine_annotation_enabled": false,
        "profile_label_improvements_pcf_label_in_post_enabled": false,
        "premium_content_api_read_enabled": false,
        "responsive_web_grok_show_grok_translated_post": false,
        "responsive_web_grok_share_attachment_enabled": false,
        "post_ctas_fetch_enabled": false,
        "responsive_web_grok_image_annotation_enabled": false,
        "responsive_web_grok_analyze_post_followups_enabled": false,
        "responsive_web_grok_analyze_button_fetch_trends_enabled": false,
        "responsive_web_grok_community_note_auto_translation_is_enabled": false,
        "rweb_video_screen_enabled": false,
        "responsive_web_grok_analysis_button_from_backend": false
    })
}

fn build_timeline_url(count: usize, cursor: Option<&str>) -> String {
    let variables = build_timeline_variables(count, cursor);
    let features = build_timeline_features();
    format!(
        "https://x.com{}?variables={}&features={}",
        HOME_TIMELINE_PATH,
        urlencoding::encode(&variables.to_string()),
        urlencoding::encode(&features.to_string())
    )
}

pub struct TwitterClient {
    http: reqwest::Client,
    #[allow(dead_code)]
    auth_token: String,
    #[allow(dead_code)]
    ct0: String,
}

#[derive(Debug, Clone)]
pub struct TimelinePage {
    pub tweets: Vec<Tweet>,
    pub next_cursor: Option<String>,
}

impl TwitterClient {
    pub fn new(auth_token: String, ct0: String) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("authority", HeaderValue::from_static("x.com"));
        headers.insert("accept", HeaderValue::from_static("*/*"));
        headers.insert(
            "accept-language",
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        headers.insert("cache-control", HeaderValue::from_static("no-cache"));
        headers.insert("pragma", HeaderValue::from_static("no-cache"));
        headers.insert("referer", HeaderValue::from_static("https://x.com/home"));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        headers.insert("x-twitter-active-user", HeaderValue::from_static("yes"));
        headers.insert(
            "x-twitter-auth-type",
            HeaderValue::from_static("OAuth2Session"),
        );
        headers.insert("x-twitter-client-language", HeaderValue::from_static("en"));

        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", BEARER_TOKEN))?,
        );
        headers.insert("x-csrf-token", HeaderValue::from_str(&ct0)?);

        let cookie_value = format!("auth_token={}; ct0={}", auth_token, ct0);
        headers.insert(COOKIE, HeaderValue::from_str(&cookie_value)?);

        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .https_only(true)
            .build()?;

        Ok(Self {
            http,
            auth_token,
            ct0,
        })
    }

    /// Generate X-Client-Transaction-Id header (required by Twitter)
    fn generate_transaction_id(method: &str, path: &str) -> Result<String> {
        let method = method.to_string();
        let path = path.to_string();

        // Must run in blocking context since x-client-transaction uses blocking reqwest
        tokio::task::block_in_place(|| {
            let blocking_client = reqwest::blocking::Client::builder()
                .user_agent(USER_AGENT)
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .build()?;

            let ct = ClientTransaction::new(&blocking_client)
                .context("Failed to create ClientTransaction")?;

            ct.generate_transaction_id(&method, &path)
                .context("Failed to generate transaction ID")
        })
    }

    /// Fetch home timeline
    pub async fn get_home_timeline(&self, count: usize) -> Result<Vec<Tweet>> {
        let page = self.get_home_timeline_page(count, None).await?;
        Ok(page.tweets)
    }

    /// Fetch home timeline page with optional cursor
    pub async fn get_home_timeline_page(
        &self,
        count: usize,
        cursor: Option<&str>,
    ) -> Result<TimelinePage> {
        let body = self.fetch_timeline(count, cursor).await?;
        self.parse_timeline_response(&body)
    }

    /// Fetch timeline and return raw JSON
    async fn fetch_timeline(&self, count: usize, cursor: Option<&str>) -> Result<Value> {
        let transaction_id = Self::generate_transaction_id("GET", HOME_TIMELINE_PATH)?;
        let url = build_timeline_url(count, cursor);

        let response = self
            .http
            .get(&url)
            .header("x-client-transaction-id", &transaction_id)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // Log full response for debugging, but don't expose to user
            logging::log_error("twitter_api", &format!("Status {}: {}", status, body));
            anyhow::bail!("Twitter API request failed (status: {})", status);
        }

        response
            .json()
            .await
            .context("Failed to parse JSON response")
    }

    /// Fetch raw home timeline JSON for debugging
    pub async fn get_home_timeline_raw(&self, count: usize) -> Result<Value> {
        self.fetch_timeline(count, None).await
    }

    /// Parse the GraphQL timeline response into Tweet structs
    fn parse_timeline_response(&self, response: &Value) -> Result<TimelinePage> {
        let mut tweets = Vec::new();
        let mut next_cursor = None;

        // HomeLatestTimeline uses: data.home.home_timeline_urt.instructions[].entries[]
        // Try both possible paths
        let instructions = response
            .get("data")
            .and_then(|d| {
                // Try home.home_timeline_urt first (HomeLatestTimeline)
                d.get("home")
                    .and_then(|h| h.get("home_timeline_urt"))
                    .and_then(|u| u.get("instructions"))
                    // Fall back to viewer.home_timeline (older format)
                    .or_else(|| {
                        d.get("viewer")
                            .and_then(|v| v.get("home_timeline"))
                            .and_then(|h| h.get("timeline"))
                            .and_then(|t| t.get("instructions"))
                    })
            })
            .and_then(|i| i.as_array())
            .context("Invalid timeline response structure")?;

        for instruction in instructions {
            if let Some(entries) = instruction.get("entries").and_then(|e| e.as_array()) {
                for entry in entries {
                    if let Some((cursor_type, cursor)) = self.parse_cursor(entry)
                        && cursor_type == "Bottom"
                    {
                        next_cursor = Some(cursor);
                    }

                    // Filter out promoted content
                    if !self.is_promoted(entry) {
                        let parsed_tweets = self.parse_entry(entry);
                        tweets.extend(parsed_tweets);
                    }
                }
            }
        }

        Ok(TimelinePage {
            tweets,
            next_cursor,
        })
    }

    fn parse_cursor(&self, entry: &Value) -> Option<(String, String)> {
        let content = entry.get("content")?;
        let typename = content.str_at("__typename");
        if typename != "TimelineTimelineCursor" && typename != "TimelineCursor" {
            return None;
        }

        let cursor_type = content
            .get("cursorType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                content
                    .get("cursor_type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })?;

        let value = content
            .get("value")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                content
                    .get("operation")
                    .and_then(|op| op.get("cursor"))
                    .and_then(|c| c.get("value"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })?;

        Some((cursor_type, value))
    }

    /// Check if an entry is promoted/ad content
    fn is_promoted(&self, entry: &Value) -> bool {
        let injection_type = entry
            .get("content")
            .and_then(|c| c.get("clientEventInfo"))
            .and_then(|i| i.get("details"))
            .and_then(|d| d.get("timelinesDetails"))
            .and_then(|t| t.get("injectionType"))
            .and_then(|i| i.as_str())
            .unwrap_or("");

        injection_type.contains("Promoted") || injection_type.contains("WhoToFollow")
    }

    /// Parse a single timeline entry into tweets (may return multiple for conversation threads)
    fn parse_entry(&self, entry: &Value) -> Vec<Tweet> {
        let Some(content) = entry.get("content") else {
            return Vec::new();
        };

        match content.str_at("__typename") {
            "TimelineTimelineItem" => self
                .parse_item_content(content.get("itemContent"))
                .into_iter()
                .collect(),
            "TimelineTimelineModule" => content
                .get("items")
                .and_then(|i| i.as_array())
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    let item_content = item.get("item").and_then(|i| i.get("itemContent"));
                    self.parse_item_content(item_content)
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Parse itemContent into a Tweet
    fn parse_item_content(&self, item_content: Option<&Value>) -> Option<Tweet> {
        let item_content = item_content?;

        if item_content.str_at("__typename") != "TimelineTweet" {
            return None;
        }

        let tweet_results = item_content.get("tweet_results")?.get("result")?;
        self.parse_tweet_result(tweet_results)
    }

    /// Parse a tweet_results object
    fn parse_tweet_result(&self, result: &Value) -> Option<Tweet> {
        let typename = result.str_at("__typename");

        // Skip unavailable tweets
        if typename == "TweetUnavailable" || typename == "TweetTombstone" {
            return None;
        }

        // Handle TweetWithVisibilityResults wrapper
        let tweet_data = if typename == "TweetWithVisibilityResults" {
            result.get("tweet")?
        } else {
            result
        };

        let legacy = tweet_data.get("legacy")?;
        let core = tweet_data.get("core")?;
        let user_results = core.get("user_results")?.get("result")?;

        if user_results.str_at("__typename") == "UserUnavailable" {
            return None;
        }

        let retweeter = self.parse_user(user_results);
        let retweeted_status = legacy
            .get("retweeted_status_result")
            .and_then(|r| r.get("result"));
        let (user, legacy, retweeted_by, is_retweet) = if let Some(retweeted) = retweeted_status {
            let retweeted_tweet = if retweeted.str_at("__typename") == "TweetWithVisibilityResults"
            {
                retweeted.get("tweet")
            } else {
                Some(retweeted)
            };
            let retweeted_tweet = retweeted_tweet?;
            let retweeted_legacy = retweeted_tweet.get("legacy")?;
            let retweeted_core = retweeted_tweet.get("core")?;
            let retweeted_user = retweeted_core.get("user_results")?.get("result")?;
            if retweeted_user.str_at("__typename") == "UserUnavailable" {
                return None;
            }
            (
                self.parse_user(retweeted_user),
                retweeted_legacy,
                Some(retweeter.screen_name.clone()),
                true,
            )
        } else {
            (retweeter, legacy, None, false)
        };

        let quoted_tweet = tweet_data
            .get("quoted_status_result")
            .and_then(|q| q.get("result"))
            .and_then(|r| self.parse_tweet_result(r))
            .map(Box::new);

        let created_at = legacy
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_str(s, "%a %b %d %H:%M:%S %z %Y").ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        Some(Tweet {
            id: tweet_data.str_at("rest_id").to_string(),
            text: legacy.str_at("full_text").to_string(),
            user,
            created_at,
            media: self.parse_media(legacy),
            retweet_count: legacy.u64_at("retweet_count"),
            like_count: legacy.u64_at("favorite_count"),
            reply_count: legacy.u64_at("reply_count"),
            is_retweet,
            retweeted_by,
            is_quote: quoted_tweet.is_some(),
            quoted_tweet,
        })
    }

    /// Parse user data from user_results
    fn parse_user(&self, user_results: &Value) -> User {
        let user_core = user_results.get("core");
        let user_legacy = user_results.get("legacy");

        // Helper to get string from core with legacy fallback
        let get_str = |key: &str, legacy_key: &str| -> String {
            user_core
                .and_then(|c| c.get(key))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    user_legacy
                        .and_then(|l| l.get(legacy_key))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("")
                .to_string()
        };

        User {
            id: user_results.str_at("rest_id").to_string(),
            name: get_str("name", "name"),
            screen_name: get_str("screen_name", "screen_name"),
            profile_image_url: user_results
                .get("avatar")
                .and_then(|a| a.get("image_url"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    user_legacy
                        .and_then(|l| l.get("profile_image_url_https"))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("")
                .to_string(),
            verified: user_legacy.map(|l| l.bool_at("verified")).unwrap_or(false),
            blue_verified: user_results.bool_at("is_blue_verified"),
        }
    }

    /// Parse media from tweet legacy data
    fn parse_media(&self, legacy: &Value) -> Vec<Media> {
        legacy
            .get("extended_entities")
            .and_then(|e| e.get("media"))
            .and_then(|m| m.as_array())
            .into_iter()
            .flatten()
            .filter_map(|item| self.parse_media_item(item))
            .collect()
    }

    /// Parse a single media item
    fn parse_media_item(&self, item: &Value) -> Option<Media> {
        let media_type = match item.str_at("type") {
            "photo" => MediaType::Photo,
            "video" => MediaType::Video,
            "animated_gif" => MediaType::Gif,
            _ => return None,
        };

        let url = if media_type == MediaType::Photo {
            item.str_at("media_url_https").to_string()
        } else {
            self.get_best_video_url(item)
        };

        let sizes = item.get("sizes").and_then(|s| s.get("large"));

        Some(Media {
            url,
            media_type,
            width: sizes.map(|s| s.u64_at("w") as u32).unwrap_or(0),
            height: sizes.map(|s| s.u64_at("h") as u32).unwrap_or(0),
            preview_url: (media_type != MediaType::Photo)
                .then(|| item.str_at("media_url_https").to_string())
                .filter(|s| !s.is_empty()),
        })
    }

    /// Get highest quality MP4 video URL from media item
    fn get_best_video_url(&self, item: &Value) -> String {
        item.get("video_info")
            .and_then(|vi| vi.get("variants"))
            .and_then(|v| v.as_array())
            .and_then(|variants| {
                variants
                    .iter()
                    .filter(|v| v.str_at("content_type") == "video/mp4")
                    .max_by_key(|v| v.u64_at("bitrate"))
                    .map(|v| v.str_at("url"))
            })
            .unwrap_or("")
            .to_string()
    }

    /// Look up a user's ID by their screen name
    pub async fn get_user_id_by_screen_name(&self, screen_name: &str) -> Result<String> {
        let variables = serde_json::json!({
            "screen_name": screen_name,
            "withSafetyModeUserFields": true
        });
        let features = serde_json::json!({
            "hidden_profile_subscriptions_enabled": true,
            "rweb_tipjar_consumption_enabled": true,
            "responsive_web_graphql_exclude_directive_enabled": true,
            "verified_phone_label_enabled": false,
            "subscriptions_verification_info_is_identity_verified_enabled": true,
            "subscriptions_verification_info_verified_since_enabled": true,
            "highlights_tweets_tab_ui_enabled": true,
            "responsive_web_twitter_article_notes_tab_enabled": true,
            "subscriptions_feature_can_gift_premium": true,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "responsive_web_graphql_timeline_navigation_enabled": true
        });

        let url = format!(
            "https://x.com{}?variables={}&features={}",
            USER_BY_SCREEN_NAME_PATH,
            urlencoding::encode(&variables.to_string()),
            urlencoding::encode(&features.to_string())
        );

        let transaction_id = Self::generate_transaction_id("GET", USER_BY_SCREEN_NAME_PATH)?;

        let response = self
            .http
            .get(&url)
            .header("x-client-transaction-id", &transaction_id)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            logging::log_error("twitter_api", &format!("User lookup {}: {}", status, body));
            anyhow::bail!("User lookup failed (status: {})", status);
        }

        let body: Value = response.json().await?;
        let user_id = body
            .get("data")
            .and_then(|d| d.get("user"))
            .and_then(|u| u.get("result"))
            .and_then(|r| r.get("rest_id"))
            .and_then(|id| id.as_str())
            .context("User not found or invalid response")?;

        Ok(user_id.to_string())
    }

    /// Get a page of accounts that a user is following
    pub async fn get_following_page(
        &self,
        user_id: &str,
        cursor: Option<&str>,
    ) -> Result<FollowingPage> {
        let mut variables = serde_json::json!({
            "userId": user_id,
            "count": 100,
            "includePromotedContent": false
        });
        if let Some(cursor) = cursor {
            variables["cursor"] = Value::String(cursor.to_string());
        }

        let features = serde_json::json!({
            "rweb_tipjar_consumption_enabled": true,
            "responsive_web_graphql_exclude_directive_enabled": true,
            "verified_phone_label_enabled": false,
            "creator_subscriptions_tweet_preview_api_enabled": true,
            "responsive_web_graphql_timeline_navigation_enabled": true,
            "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
            "communities_web_enable_tweet_community_results_fetch": true,
            "c9s_tweet_anatomy_moderator_badge_enabled": true,
            "articles_preview_enabled": true,
            "responsive_web_edit_tweet_api_enabled": true,
            "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
            "view_counts_everywhere_api_enabled": true,
            "longform_notetweets_consumption_enabled": true,
            "responsive_web_twitter_article_tweet_consumption_enabled": true,
            "tweet_awards_web_tipping_enabled": false,
            "creator_subscriptions_quote_tweet_preview_enabled": false,
            "freedom_of_speech_not_reach_fetch_enabled": true,
            "standardized_nudges_misinfo": true,
            "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
            "rweb_video_timestamps_enabled": true,
            "longform_notetweets_rich_text_read_enabled": true,
            "longform_notetweets_inline_media_enabled": true,
            "responsive_web_enhance_cards_enabled": false
        });

        let url = format!(
            "https://x.com{}?variables={}&features={}",
            FOLLOWING_PATH,
            urlencoding::encode(&variables.to_string()),
            urlencoding::encode(&features.to_string())
        );

        let transaction_id = Self::generate_transaction_id("GET", FOLLOWING_PATH)?;

        let response = self
            .http
            .get(&url)
            .header("x-client-transaction-id", &transaction_id)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            logging::log_error(
                "twitter_api",
                &format!("Following lookup {}: {}", status, body),
            );
            anyhow::bail!("Following lookup failed (status: {})", status);
        }

        let body: Value = response.json().await?;
        self.parse_following_response(&body)
    }

    /// Parse following response into users and cursor
    fn parse_following_response(&self, response: &Value) -> Result<FollowingPage> {
        let mut users = Vec::new();
        let mut next_cursor = None;

        let instructions = response
            .get("data")
            .and_then(|d| d.get("user"))
            .and_then(|u| u.get("result"))
            .and_then(|r| r.get("timeline"))
            .and_then(|t| t.get("timeline"))
            .and_then(|t| t.get("instructions"))
            .and_then(|i| i.as_array())
            .context("Invalid following response structure")?;

        for instruction in instructions {
            let Some(entries) = instruction.get("entries").and_then(|e| e.as_array()) else {
                continue;
            };

            for entry in entries {
                // Check for cursor
                let entry_id = entry.str_at("entryId");
                if entry_id.starts_with("cursor-bottom") {
                    if let Some(cursor) = entry
                        .get("content")
                        .and_then(|c| c.get("value"))
                        .and_then(|v| v.as_str())
                    {
                        next_cursor = Some(cursor.to_string());
                    }
                    continue;
                }

                // Parse user entry
                if !entry_id.starts_with("user-") {
                    continue;
                }
                if let Some(user) = self.parse_following_entry(entry) {
                    users.push(user);
                }
            }
        }

        Ok(FollowingPage { users, next_cursor })
    }

    /// Parse a single user entry from following response
    fn parse_following_entry(&self, entry: &Value) -> Option<FollowingUser> {
        let user_results = entry
            .get("content")
            .and_then(|c| c.get("itemContent"))
            .and_then(|i| i.get("user_results"))
            .and_then(|u| u.get("result"))?;

        if user_results.str_at("__typename") == "UserUnavailable" {
            return None;
        }

        let rest_id = user_results.str_at("rest_id").to_string();
        if rest_id.is_empty() {
            return None;
        }

        let legacy = user_results.get("legacy")?;

        Some(FollowingUser {
            id: rest_id,
            screen_name: legacy.str_at("screen_name").to_string(),
            name: legacy.str_at("name").to_string(),
            following: legacy.bool_at("following"),
        })
    }

    /// Follow a user by their ID
    pub async fn follow_user(&self, user_id: &str) -> Result<()> {
        let transaction_id = Self::generate_transaction_id("POST", CREATE_FRIENDSHIP_PATH)?;

        let url = format!("https://x.com{}", CREATE_FRIENDSHIP_PATH);

        let response = self
            .http
            .post(&url)
            .header("x-client-transaction-id", &transaction_id)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!(
                "include_profile_interstitial_type=1&skip_status=true&user_id={}",
                user_id
            ))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            logging::log_error(
                "twitter_api",
                &format!("Follow action {}: {}", status, body),
            );
            anyhow::bail!("Follow action failed (status: {})", status);
        }

        Ok(())
    }
}

/// A user from the following list
#[derive(Debug, Clone)]
pub struct FollowingUser {
    pub id: String,
    pub screen_name: String,
    pub name: String,
    pub following: bool,
}

/// A page of following results
#[derive(Debug)]
pub struct FollowingPage {
    pub users: Vec<FollowingUser>,
    pub next_cursor: Option<String>,
}
