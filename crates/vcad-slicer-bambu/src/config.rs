//! Printer credentials file (`~/.config/vcad/printers.toml`).
//!
//! ```toml
//! [[printer]]
//! name = "p1s"
//! model = "P1S"
//! ip = "192.168.1.50"
//! serial = "01P00A000000000"
//! access_code = "12345678"
//! ```
//!
//! The access code is a LAN credential, so the loader refuses a file that
//! group or others can read (`chmod 600`), and `Debug` never prints it.
//! The file lives outside every repo and is never committed.

use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{BambuError, Result};

/// One configured printer.
#[derive(Clone, Deserialize)]
pub struct PrinterEntry {
    /// Short name used on the command line (`--printer p1s`).
    pub name: String,
    /// Model as shown on the printer, e.g. `P1S`.
    #[serde(default = "default_model")]
    pub model: String,
    /// LAN IP address.
    pub ip: IpAddr,
    /// Serial number (Settings ▸ Device on the printer).
    pub serial: String,
    /// LAN access code (Settings ▸ WLAN on the printer).
    pub access_code: String,
}

fn default_model() -> String {
    "P1S".into()
}

impl fmt::Debug for PrinterEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrinterEntry")
            .field("name", &self.name)
            .field("model", &self.model)
            .field("ip", &self.ip)
            .field("serial", &self.serial)
            .field("access_code", &"<redacted>")
            .finish()
    }
}

impl PrinterEntry {
    /// Bambu's internal model id, as written in a sliced file's
    /// `slice_info.config` (`printer_model_id`). `None` for models we have
    /// no verified id for — the print gate then refuses.
    pub fn model_id(&self) -> Option<&'static str> {
        model_id_for(&self.model)
    }
}

/// Bambu model id for a marketing model name. Ids from BambuStudio's
/// `resources/profiles/BBL/machine/*.json` (`model_id`).
pub fn model_id_for(model: &str) -> Option<&'static str> {
    match model.to_ascii_uppercase().as_str() {
        "P1S" => Some("C12"),
        "P1P" => Some("C11"),
        "X1C" | "X1 CARBON" => Some("BL-P001"),
        "A1" => Some("N2S"),
        "A1 MINI" | "A1MINI" => Some("N1"),
        _ => None,
    }
}

#[derive(Deserialize)]
struct PrintersFile {
    #[serde(default)]
    printer: Vec<PrinterEntry>,
}

/// Default location: `$VCAD_PRINTERS`, else `~/.config/vcad/printers.toml`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VCAD_PRINTERS") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/vcad/printers.toml"))
}

/// Parse a printers file's contents (no permission check).
pub fn parse(text: &str) -> Result<Vec<PrinterEntry>> {
    let file: PrintersFile = toml::from_str(text)
        .map_err(|e| BambuError::InvalidResponse(format!("printers.toml: {e}")))?;
    for p in &file.printer {
        if p.access_code.trim().is_empty() || p.serial.trim().is_empty() {
            return Err(BambuError::InvalidResponse(format!(
                "printers.toml: printer `{}` needs serial and access_code",
                p.name
            )));
        }
    }
    Ok(file.printer)
}

/// Load and permission-check a printers file.
pub fn load(path: &Path) -> Result<Vec<PrinterEntry>> {
    check_private(path)?;
    let text = std::fs::read_to_string(path)?;
    parse(&text)
}

/// Pick a printer by name; with no name, the only one configured.
pub fn select<'a>(printers: &'a [PrinterEntry], name: Option<&str>) -> Result<&'a PrinterEntry> {
    match name {
        Some(n) => printers
            .iter()
            .find(|p| p.name == n)
            .ok_or_else(|| BambuError::PrinterNotFound(format!("no printer named `{n}`"))),
        None => match printers {
            [one] => Ok(one),
            [] => Err(BambuError::PrinterNotFound(
                "printers.toml lists no printers".into(),
            )),
            _ => Err(BambuError::PrinterNotFound(
                "several printers configured; pass --printer <name>".into(),
            )),
        },
    }
}

#[cfg(unix)]
fn check_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(BambuError::AuthenticationFailed(format!(
            "{} is readable by others (mode {:o}); run `chmod 600 {}`",
            path.display(),
            mode & 0o777,
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_private(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[printer]]
name = "p1s"
ip = "192.168.1.50"
serial = "01P00A000000000"
access_code = "12345678"
"#;

    #[test]
    fn parses_and_defaults_model() {
        let ps = parse(SAMPLE).unwrap();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].model, "P1S");
        assert_eq!(ps[0].model_id(), Some("C12"));
        assert_eq!(select(&ps, None).unwrap().name, "p1s");
        assert!(select(&ps, Some("x1")).is_err());
    }

    #[test]
    fn debug_never_prints_the_access_code() {
        let ps = parse(SAMPLE).unwrap();
        let s = format!("{:?}", ps[0]);
        assert!(!s.contains("12345678"), "{s}");
        assert!(s.contains("redacted"));
    }

    #[test]
    fn empty_access_code_is_refused() {
        let bad = SAMPLE.replace("12345678", "");
        assert!(parse(&bad).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn world_readable_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("printers.toml");
        std::fs::write(&p, SAMPLE).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("chmod 600"), "{err}");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(load(&p).unwrap().len(), 1);
    }
}
