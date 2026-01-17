use anyhow::{Context, Result};
use filetime::FileTime;
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use crate::config::Config;
use crate::twitter::USER_AGENT;

/// Manages image loading and caching for the TUI
/// Uses two-level caching:
/// 1. In-memory cache (HashMap) for fast access during session
/// 2. LRU disk cache (~/.cache/twit/images/) for persistence across sessions
pub struct ImageManager {
    picker: Picker,
    cache: HashMap<String, StatefulProtocol>,
    failed: HashSet<String>,
    http: reqwest::Client,
    disk_cache_dir: PathBuf,
    max_cache_bytes: u64,
}

impl ImageManager {
    /// Create a new image manager with specified cache size limit in MB
    pub fn new_with_cache_limit(max_cache_mb: u64) -> Result<Self> {
        let picker =
            Picker::from_query_stdio().context("Failed to query terminal graphics capabilities")?;

        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .build()?;

        let disk_cache_dir = Config::cache_dir().join("images");
        fs::create_dir_all(&disk_cache_dir).ok();

        Ok(Self {
            picker,
            cache: HashMap::new(),
            failed: HashSet::new(),
            http,
            disk_cache_dir,
            max_cache_bytes: max_cache_mb * 1024 * 1024,
        })
    }

    /// Generate a cache filename from URL (hash to avoid filesystem issues)
    fn cache_filename(&self, url: &str) -> PathBuf {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        let hash = hasher.finish();

        let ext = if url.contains(".png") {
            "png"
        } else if url.contains(".gif") {
            "gif"
        } else {
            "jpg"
        };

        self.disk_cache_dir.join(format!("{:x}.{}", hash, ext))
    }

    /// Touch a file to update its access time (for LRU tracking)
    fn touch_file(path: &PathBuf) {
        let now = FileTime::now();
        let _ = filetime::set_file_mtime(path, now);
    }

    /// Get total size of disk cache in bytes
    fn disk_cache_size(&self) -> u64 {
        fs::read_dir(&self.disk_cache_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Evict oldest files from disk cache until under the size limit
    fn evict_if_needed(&self) {
        let current_size = self.disk_cache_size();
        if current_size <= self.max_cache_bytes {
            return;
        }

        // Collect files with their modification times
        let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> =
            fs::read_dir(&self.disk_cache_dir)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    let meta = entry.metadata().ok()?;
                    let mtime = meta.modified().ok()?;
                    Some((path, meta.len(), mtime))
                })
                .collect();

        // Sort by mtime ascending (oldest first)
        files.sort_by_key(|(_, _, mtime)| *mtime);

        // Delete oldest files until under limit
        let mut size = current_size;
        for (path, file_size, _) in files {
            if size <= self.max_cache_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                size = size.saturating_sub(file_size);
            }
        }
    }

    /// Load an image from URL, using disk cache if available
    pub async fn load_image(&mut self, url: &str) -> Result<()> {
        if self.cache.contains_key(url) || self.failed.contains(url) {
            return Ok(());
        }

        let cache_path = self.cache_filename(url);

        let bytes = if cache_path.exists() {
            Self::touch_file(&cache_path);
            fs::read(&cache_path).ok()
        } else {
            None
        };

        let bytes = match bytes {
            Some(b) => b,
            None => {
                let response = self.http.get(url).send().await.map_err(|e| {
                    self.failed.insert(url.to_string());
                    anyhow::Error::from(e)
                })?;
                let fetched_bytes = response.bytes().await.map_err(|e| {
                    self.failed.insert(url.to_string());
                    anyhow::Error::from(e)
                })?;
                let fetched_bytes = fetched_bytes.to_vec();

                self.evict_if_needed();

                if let Some(parent) = cache_path.parent() {
                    fs::create_dir_all(parent).ok();
                }
                fs::write(&cache_path, &fetched_bytes).ok();

                fetched_bytes
            }
        };

        let img = image::load_from_memory(&bytes).map_err(|e| {
            if cache_path.exists() {
                let _ = fs::remove_file(&cache_path);
            }
            self.failed.insert(url.to_string());
            anyhow::anyhow!("Failed to decode image: {}", e)
        })?;

        let protocol = self.picker.new_resize_protocol(img);
        self.cache.insert(url.to_string(), protocol);

        Ok(())
    }

    /// Get a mutable reference to the cached image protocol for rendering
    pub fn get_protocol(&mut self, url: &str) -> Option<&mut StatefulProtocol> {
        self.cache.get_mut(url)
    }

    /// Check if an image is loaded or was attempted (includes failures to avoid retry spam)
    pub fn is_in_memory(&self, url: &str) -> bool {
        self.cache.contains_key(url) || self.failed.contains(url)
    }
}
