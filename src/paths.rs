//! Shared filesystem path helpers.
//!
//! Centralises path resolution that was previously duplicated across each
//! CLI subcommand and `extractor::output::OutputPaths::resolve`. See M5
//! design spec §3.9.

use std::path::PathBuf;

/// Where Rover persists its SQLite cache, logs, and other per-user state.
///
/// Resolution order:
/// 1. `ROVER_DATA_DIR` environment variable, if set and non-empty.
/// 2. `dirs::data_local_dir()/rover` (platform default).
/// 3. `./.rover` (last-resort relative fallback; only hit when the platform
///    helper fails, which is rare on supported OSes).
pub fn data_dir() -> PathBuf {
    if let Ok(env) = std::env::var("ROVER_DATA_DIR")
        && !env.is_empty()
    {
        return PathBuf::from(env);
    }
    dirs::data_local_dir()
        .map(|p| p.join("rover"))
        .unwrap_or_else(|| PathBuf::from("./.rover"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env var manipulation across tests within the file.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_var_wins_over_platform_default() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: tests in this module hold ENV_LOCK; no other thread reads
        // ROVER_DATA_DIR concurrently.
        unsafe { std::env::set_var("ROVER_DATA_DIR", "/tmp/rover-test-data") };
        let p = data_dir();
        unsafe { std::env::remove_var("ROVER_DATA_DIR") };
        assert_eq!(p, PathBuf::from("/tmp/rover-test-data"));
    }

    #[test]
    fn empty_env_var_falls_through() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("ROVER_DATA_DIR", "") };
        let p = data_dir();
        unsafe { std::env::remove_var("ROVER_DATA_DIR") };
        assert!(p.ends_with("rover") || p.to_string_lossy().contains(".rover"));
    }

    #[test]
    fn unset_env_falls_through_to_platform_default() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("ROVER_DATA_DIR") };
        let p = data_dir();
        // dirs::data_local_dir() is Some on all CI targets; assertion is loose
        // because the exact path varies (Linux: ~/.local/share/rover, macOS:
        // ~/Library/Application Support/rover, Windows: AppData/Local/rover).
        assert!(p.ends_with("rover"));
    }
}
