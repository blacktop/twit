use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::Config;

/// Maximum log file size before rotation (1 MB)
const MAX_LOG_SIZE: u64 = 1024 * 1024;
/// Number of rotated log files to keep
const MAX_LOG_FILES: u32 = 3;

/// Global flag to enable/disable debug logging
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable debug logging for the current session
pub fn enable_debug() {
    DEBUG_ENABLED.store(true, Ordering::SeqCst);
}

pub fn log_error(context: &str, message: &str) {
    log_message("ERROR", context, message);
}

pub fn log_info(context: &str, message: &str) {
    log_message("INFO", context, message);
}

fn log_message(level: &str, context: &str, message: &str) {
    // Only log when debug mode is enabled
    if !DEBUG_ENABLED.load(Ordering::SeqCst) {
        return;
    }

    let path = Config::log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Rotate log if it exceeds max size
    rotate_if_needed(&path);

    // Set restrictive permissions on new log files
    let file_exists = path.exists();
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(_) => return,
    };

    // Set permissions on newly created log file
    if !file_exists {
        set_log_permissions(&path);
    }

    let timestamp = Utc::now().to_rfc3339();
    let _ = writeln!(file, "[{}] {} {}: {}", timestamp, level, context, message);
}

/// Rotate the log file if it exceeds MAX_LOG_SIZE
fn rotate_if_needed(path: &Path) {
    let size = match fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(_) => return, // File doesn't exist yet
    };

    if size < MAX_LOG_SIZE {
        return;
    }

    // Rotate: .3 -> delete, .2 -> .3, .1 -> .2, current -> .1
    for i in (1..MAX_LOG_FILES).rev() {
        let from = path.with_extension(format!("log.{}", i));
        let to = path.with_extension(format!("log.{}", i + 1));
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }

    // Rotate current log to .1
    let rotated = path.with_extension("log.1");
    let _ = fs::rename(path, &rotated);
}

/// Set restrictive permissions on log files (owner read/write only)
#[cfg(unix)]
fn set_log_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    let _ = std::fs::set_permissions(path, perms);
}

#[cfg(not(unix))]
fn set_log_permissions(_path: &Path) {
    // No-op on non-Unix systems
}
