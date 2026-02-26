//! Bambu printer discovery via SSDP.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

use crate::error::{BambuError, Result};

/// Information about a discovered printer.
#[derive(Debug, Clone)]
pub struct PrinterInfo {
    /// Printer IP address.
    pub ip: IpAddr,
    /// Printer serial number.
    pub serial: String,
    /// Printer model (e.g., "X1C", "P1S", "A1").
    pub model: String,
    /// Printer name.
    pub name: String,
    /// Firmware version.
    pub firmware: Option<String>,
}

/// Discover Bambu printers on the local network using SSDP.
///
/// This sends an SSDP M-SEARCH request and waits for responses.
pub fn discover_printers(timeout: Duration) -> Result<Vec<PrinterInfo>> {
    let socket =
        UdpSocket::bind("0.0.0.0:0").map_err(|e| BambuError::DiscoveryError(e.to_string()))?;

    socket
        .set_read_timeout(Some(timeout))
        .map_err(|e| BambuError::DiscoveryError(e.to_string()))?;

    // SSDP multicast address
    let multicast_addr: SocketAddr = "239.255.255.250:1990".parse().unwrap();

    // M-SEARCH request for Bambu printers
    let search_request = concat!(
        "M-SEARCH * HTTP/1.1\r\n",
        "HOST: 239.255.255.250:1990\r\n",
        "MAN: \"ssdp:discover\"\r\n",
        "MX: 3\r\n",
        "ST: urn:bambulab-com:device:3dprinter:1\r\n",
        "\r\n"
    );

    // Send discovery request
    socket
        .send_to(search_request.as_bytes(), multicast_addr)
        .map_err(|e| BambuError::DiscoveryError(e.to_string()))?;

    let mut printers = Vec::new();
    let mut buf = [0u8; 2048];

    // Receive responses
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, addr)) => {
                if let Ok(response) = std::str::from_utf8(&buf[..len]) {
                    if let Some(info) = parse_ssdp_response(response, addr.ip()) {
                        // Avoid duplicates
                        if !printers
                            .iter()
                            .any(|p: &PrinterInfo| p.serial == info.serial)
                        {
                            printers.push(info);
                        }
                    }
                }
            }
            Err(e) => {
                // Timeout or other error
                if e.kind() != std::io::ErrorKind::WouldBlock
                    && e.kind() != std::io::ErrorKind::TimedOut
                {
                    return Err(BambuError::DiscoveryError(e.to_string()));
                }
                break;
            }
        }
    }

    Ok(printers)
}

/// Parse an SSDP response into printer info.
fn parse_ssdp_response(response: &str, ip: IpAddr) -> Option<PrinterInfo> {
    // Check if this is a Bambu printer response
    if !response.contains("bambulab") && !response.contains("Bambu") {
        return None;
    }

    let mut serial = None;
    let mut model = None;
    let mut name = None;
    let mut firmware = None;

    for line in response.lines() {
        let line = line.trim();

        if let Some(value) = line.strip_prefix("USN:") {
            // USN format: uuid:SERIAL::urn:bambulab-com:device:3dprinter:1
            let value = value.trim();
            if let Some(uuid_part) = value.strip_prefix("uuid:") {
                if let Some(sn) = uuid_part.split("::").next() {
                    serial = Some(sn.trim().to_string());
                }
            }
        } else if let Some(value) = line.strip_prefix("DevModel.bambu.com:") {
            model = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("DevName.bambu.com:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("DevVersion.bambu.com:") {
            firmware = Some(value.trim().to_string());
        }
    }

    // Require at least serial and model
    let serial = serial?;
    let model = model.unwrap_or_else(|| "Unknown".into());
    let name = name.unwrap_or_else(|| format!("Bambu {}", model));

    Some(PrinterInfo {
        ip,
        serial,
        model,
        name,
        firmware,
    })
}

/// Discover printers asynchronously.
pub async fn discover_printers_async(timeout: Duration) -> Result<Vec<PrinterInfo>> {
    // Run synchronous discovery in a blocking task
    tokio::task::spawn_blocking(move || discover_printers(timeout))
        .await
        .map_err(|e| BambuError::DiscoveryError(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssdp_response() {
        let response = r#"HTTP/1.1 200 OK
CACHE-CONTROL: max-age=1800
USN: uuid:00M00A2B012345::urn:bambulab-com:device:3dprinter:1
DevModel.bambu.com: X1C
DevName.bambu.com: My Printer
DevVersion.bambu.com: 01.07.00.00
"#;
        let ip: IpAddr = "192.168.1.100".parse().unwrap();
        let info = parse_ssdp_response(response, ip).unwrap();

        assert_eq!(info.serial, "00M00A2B012345");
        assert_eq!(info.model, "X1C");
        assert_eq!(info.name, "My Printer");
        assert_eq!(info.ip.to_string(), "192.168.1.100");
    }
}
