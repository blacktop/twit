use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::twitter::Tweet;

/// Set restrictive file permissions (owner read/write only) on Unix systems
#[cfg(unix)]
fn set_restrictive_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("Failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_restrictive_permissions(_path: &Path) -> Result<()> {
    Ok(()) // No-op on non-Unix systems
}

const TWEETS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("tweets");
const META_TABLE: TableDefinition<&str, u64> = TableDefinition::new("meta");
const META_LAST_SAVE: &str = "last_save";
const META_LATEST_TWEET_TS: &str = "latest_tweet_ts";

pub struct TweetCache {
    db: Database,
}

impl TweetCache {
    /// Open or create the tweet cache database
    pub fn open() -> Result<Self> {
        let cache_dir = Config::cache_dir();
        std::fs::create_dir_all(&cache_dir).with_context(|| {
            format!("Failed to create cache directory: {}", cache_dir.display())
        })?;

        let db_path = cache_dir.join("tweets.redb");

        // Try to open the database, delete and recreate if format version mismatch
        let db = match Database::create(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("Cannot acquire lock")
                    || err_str.contains("already open")
                    || err_str.contains("locked")
                {
                    anyhow::bail!(
                        "Another instance of twit is already running.\n\
                         Only one instance can run at a time (database is locked)."
                    );
                } else if err_str.contains("Manual upgrade required")
                    || err_str.contains("file format version")
                {
                    // Delete old database and create fresh one
                    eprintln!(
                        "Cache database format outdated, resetting: {}",
                        db_path.display()
                    );
                    std::fs::remove_file(&db_path).ok();
                    Database::create(&db_path).with_context(|| {
                        format!("Failed to create cache database: {}", db_path.display())
                    })?
                } else {
                    return Err(e).with_context(|| {
                        format!("Failed to open cache database: {}", db_path.display())
                    });
                }
            }
        };

        // Set restrictive permissions on the database file (contains cached tweets)
        set_restrictive_permissions(&db_path)?;

        // Ensure tables exist
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(TWEETS_TABLE)?;
            let _ = write_txn.open_table(META_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db })
    }

    /// Get all cached tweets, sorted by created_at descending
    pub fn get_tweets(&self) -> Result<Vec<Tweet>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TWEETS_TABLE)?;

        let mut tweets = Vec::new();
        for result in table.iter()? {
            let (_, value) = result?;
            if let Ok(tweet) = serde_json::from_slice::<Tweet>(value.value()) {
                tweets.push(tweet);
            }
        }

        // Sort by created_at descending (newest first)
        tweets.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(tweets)
    }

    /// Save tweets to the cache (merges with existing)
    pub fn save_tweets(&self, tweets: &[Tweet]) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TWEETS_TABLE)?;
            for tweet in tweets {
                let key = tweet.id.as_str();
                let value = serde_json::to_vec(tweet)?;
                table.insert(key, value.as_slice())?;
            }

            // Update last save timestamp
            let mut meta = write_txn.open_table(META_TABLE)?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            meta.insert(META_LAST_SAVE, now)?;

            if let Some(latest_ts) = latest_timestamp_secs(tweets) {
                let existing = meta.get(META_LATEST_TWEET_TS)?.map(|v| v.value());
                let next = existing
                    .map(|value| value.max(latest_ts))
                    .unwrap_or(latest_ts);
                meta.insert(META_LATEST_TWEET_TS, next)?;
            }
        }
        write_txn.commit()?;

        Ok(())
    }

    /// Remove tweets older than the given TTL
    pub fn clear_old(&self, ttl: Duration) -> Result<usize> {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(ttl).unwrap_or_default();
        let mut removed = 0;

        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(TWEETS_TABLE)?;

            // Collect keys to remove
            let mut to_remove = Vec::new();
            for result in table.iter()? {
                let (key, value) = result?;
                if let Ok(tweet) = serde_json::from_slice::<Tweet>(value.value())
                    && tweet.created_at < cutoff
                {
                    to_remove.push(key.value().to_string());
                }
            }

            // Remove old tweets
            for key in to_remove {
                table.remove(key.as_str())?;
                removed += 1;
            }
        }
        write_txn.commit()?;

        Ok(removed)
    }

    /// Get the most recent tweet timestamp seen in the cache
    pub fn latest_tweet_time(&self) -> Result<Option<DateTime<Utc>>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(META_TABLE)?;

        if let Some(value) = table.get(META_LATEST_TWEET_TS)? {
            let secs = value.value();
            let system_time = UNIX_EPOCH + Duration::from_secs(secs);
            Ok(Some(DateTime::<Utc>::from(system_time)))
        } else {
            Ok(None)
        }
    }
}

fn latest_timestamp_secs(tweets: &[Tweet]) -> Option<u64> {
    tweets
        .iter()
        .filter_map(|tweet| u64::try_from(tweet.created_at.timestamp()).ok())
        .max()
}
