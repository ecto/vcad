//! FTPS for Bambu Lab printers.
//!
//! Bambu printers accept files via FTPS on port 990 (implicit TLS), user
//! `bblp`, password = the LAN access code. The printer's certificate is
//! signed by Bambu's per-device CA (see [`crate::tls`]).

use std::io::Cursor;
use std::net::IpAddr;

use suppaftp::RustlsConnector;
use suppaftp::RustlsFtpStream;

use crate::error::{BambuError, Result};

/// Upload a 3MF file to a Bambu printer via FTPS.
///
/// The file is uploaded to the printer's SD card at the root directory.
/// After upload, use MQTT to send a PrintStart command referencing the filename.
pub async fn upload_3mf(ip: IpAddr, access_code: &str, filename: &str, data: &[u8]) -> Result<()> {
    let access_code = access_code.to_string();
    let filename = filename.to_string();
    let data = data.to_vec();

    tokio::task::spawn_blocking(move || upload_blocking(ip, &access_code, &filename, &data))
        .await
        .map_err(|e| BambuError::PrintError(format!("FTPS task panicked: {}", e)))?
}

/// Open an authenticated FTPS session.
fn login(ip: IpAddr, access_code: &str) -> Result<RustlsFtpStream> {
    let addr = format!("{}:990", ip);
    let connector: RustlsConnector = crate::tls::printer_client_config().into();
    let mut ftp = RustlsFtpStream::connect_secure_implicit(&addr, connector, &ip.to_string())
        .map_err(|e| BambuError::ConnectionFailed(format!("FTPS connect failed: {}", e)))?;
    ftp.login("bblp", access_code)
        .map_err(|e| BambuError::AuthenticationFailed(format!("FTPS login failed: {}", e)))?;
    Ok(ftp)
}

/// Blocking upload of `data` to `filename` at the SD card root.
pub fn upload_blocking(ip: IpAddr, access_code: &str, filename: &str, data: &[u8]) -> Result<()> {
    let mut ftp = login(ip, access_code)?;
    let mut reader = Cursor::new(data);
    ftp.put_file(filename, &mut reader)
        .map_err(|e| BambuError::PrintError(format!("FTPS upload failed: {}", e)))?;
    ftp.quit()
        .map_err(|e| BambuError::PrintError(format!("FTPS quit failed: {}", e)))?;
    Ok(())
}

/// Blocking size query for `filename`; `None` when it doesn't exist.
pub fn size_blocking(ip: IpAddr, access_code: &str, filename: &str) -> Result<Option<u64>> {
    let mut ftp = login(ip, access_code)?;
    let size = ftp.size(filename).ok().map(|s| s as u64);
    let _ = ftp.quit();
    Ok(size)
}

/// Blocking delete of `filename` at the SD card root.
pub fn delete_blocking(ip: IpAddr, access_code: &str, filename: &str) -> Result<()> {
    let mut ftp = login(ip, access_code)?;
    ftp.rm(filename)
        .map_err(|e| BambuError::PrintError(format!("FTPS delete failed: {}", e)))?;
    let _ = ftp.quit();
    Ok(())
}
