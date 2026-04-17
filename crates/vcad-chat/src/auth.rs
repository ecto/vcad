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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("device-code login timed out — no response after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
    #[error("device-code login cancelled")]
    Cancelled,
}

/// Absolute path to the token file.
pub fn token_path() -> Result<PathBuf, AuthError> {
    let dirs =
        directories::ProjectDirs::from("io", "vcad", "vcad").ok_or(AuthError::NoConfigDir)?;
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

// ---------------------------------------------------------------------------
// Device-code browser flow
// ---------------------------------------------------------------------------

/// Default URLs for the device-code flow. Overridable via env for local
/// testing against `vercel dev`.
const DEFAULT_CLI_AUTH_PAGE: &str = "https://vcad.io/cli-auth";
const DEFAULT_CLI_AUTH_API: &str = "https://vcad.io/api/cli-auth";
/// How long to wait for the user to complete the browser flow before
/// giving up.
const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_secs(300);
/// Interval between polls. Keep this comfortably under the cache TTL
/// the server is expected to enforce on stored code→token entries.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A one-time device code tied to a pending browser login. Generated
/// client-side; the browser page uses it to look up the right slot
/// when POSTing the completed token.
#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub code: String,
    pub login_url: String,
}

/// Generate a random device code. Uses OS time + process id as entropy
/// — good enough for a short-lived one-time code, no new crate needed.
pub fn generate_device_code() -> DeviceCode {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    // 16 hex chars of time + 8 of pid → 24-char code. Plenty for a
    // short-lived server-side lookup.
    let code = format!("{:016x}{:08x}", nanos as u64, pid);
    let login_url = format!("{}?code={}", cli_auth_page_url(), code);
    DeviceCode { code, login_url }
}

/// Base page URL the browser lands on. Uses `VCAD_CLI_AUTH_PAGE` when set.
fn cli_auth_page_url() -> String {
    std::env::var("VCAD_CLI_AUTH_PAGE").unwrap_or_else(|_| DEFAULT_CLI_AUTH_PAGE.to_string())
}

/// Base API URL the TUI polls. Uses `VCAD_CLI_AUTH_API` when set.
fn cli_auth_api_url() -> String {
    std::env::var("VCAD_CLI_AUTH_API").unwrap_or_else(|_| DEFAULT_CLI_AUTH_API.to_string())
}

/// Open the login URL in the user's default browser. Best-effort —
/// returns `Ok(())` even when no opener is found, so callers can
/// print the URL as a fallback.
pub fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let (cmd, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let (cmd, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let (cmd, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let (cmd, args): (&str, Vec<&str>) = ("echo", vec![url]);

    std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// Poll `GET /api/cli-auth?code=X` until a token arrives or the timeout
/// expires. This is a blocking helper for the `vcad login` subcommand —
/// it runs on its own tokio runtime so the caller doesn't need one.
pub fn poll_for_token(code: &str, timeout: Option<Duration>) -> Result<Token, AuthError> {
    let timeout = timeout.unwrap_or(DEFAULT_POLL_TIMEOUT);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(std::io::Error::other)?;
    runtime.block_on(poll_for_token_async(code, timeout))
}

async fn poll_for_token_async(code: &str, timeout: Duration) -> Result<Token, AuthError> {
    let client = reqwest::Client::new();
    let url = format!("{}?code={}", cli_auth_api_url(), code);
    let deadline = Instant::now() + timeout;

    loop {
        if Instant::now() >= deadline {
            return Err(AuthError::Timeout {
                timeout_secs: timeout.as_secs(),
            });
        }

        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => return Err(AuthError::Network(e)),
        };

        match resp.status().as_u16() {
            200 => {
                let token: Token = resp.json().await?;
                return Ok(token);
            }
            404 | 408 => {
                // Not yet — keep polling.
                tokio::time::sleep(DEFAULT_POLL_INTERVAL).await;
            }
            status => {
                let body = resp.text().await.unwrap_or_default();
                return Err(AuthError::Io(std::io::Error::other(format!(
                    "cli-auth poll failed: HTTP {status} {body}"
                ))));
            }
        }
    }
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
