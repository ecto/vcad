//! Token storage for the Supabase JWT used to authenticate `/api/chat`.
//!
//! Writes to `$XDG_CONFIG_HOME/vcad/token.json` (or the OS-appropriate
//! equivalent via the `directories` crate). The file is marked `0600` on
//! unix so it can't be read by other local users.
//!
//! The device-code browser flow that *acquires* the token lives in M5
//! (`vcad login` subcommand + a new `api/cli-auth.ts` endpoint). For now
//! we expose just `load_token` / `save_token` / `clear_token` so that the
//! TUI can persist a token supplied via `--token` or env var.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A persisted Supabase session — mirrors the fields the web app stores in
/// its auth store. `access_token` is the JWT we pass in the Authorization
/// header; `refresh_token` is used by M5 to renew without re-prompting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds when `access_token` expires. Used later for refresh.
    pub expires_at: Option<i64>,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("no config directory on this platform")]
    NoConfigDir,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("token serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Absolute path to the token file.
pub fn token_path() -> Result<PathBuf, AuthError> {
    let dirs = directories::ProjectDirs::from("io", "vcad", "vcad").ok_or(AuthError::NoConfigDir)?;
    Ok(dirs.config_dir().join("token.json"))
}

/// Load a persisted token, or `Ok(None)` if the file doesn't exist yet.
pub fn load_token() -> Result<Option<Token>, AuthError> {
    let path = token_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    let token: Token = serde_json::from_slice(&bytes)?;
    Ok(Some(token))
}

/// Persist a token to disk. Creates the parent directory if needed and
/// tightens permissions to `0600` on unix.
pub fn save_token(token: &Token) -> Result<(), AuthError> {
    let path = token_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(token)?;
    fs::write(&path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

/// Remove the token file if it exists. Safe to call when there's no token.
pub fn clear_token() -> Result<(), AuthError> {
    let path = token_path()?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trips_through_json() {
        let t = Token {
            access_token: "abc.def.ghi".into(),
            refresh_token: Some("rtok".into()),
            expires_at: Some(1_234_567_890),
        };
        let bytes = serde_json::to_vec(&t).unwrap();
        let back: Token = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.access_token, "abc.def.ghi");
        assert_eq!(back.refresh_token.as_deref(), Some("rtok"));
        assert_eq!(back.expires_at, Some(1_234_567_890));
    }

    #[test]
    fn token_path_has_vcad_segment() {
        let path = token_path().expect("needs a config dir in test env");
        let s = path.to_string_lossy();
        assert!(s.contains("vcad"), "expected 'vcad' in {s}");
        assert!(s.ends_with("token.json"));
    }
}
