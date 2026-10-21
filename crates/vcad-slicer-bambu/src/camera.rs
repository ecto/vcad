//! P1/A1-series camera: JPEG frames over TLS on port 6000.
//!
//! The X1 serves RTSP; the P1 and A1 serve a bespoke stream instead. After
//! TLS, the client sends one 80-byte auth packet:
//!
//! | bytes | value |
//! |---|---|
//! | 0..4   | `0x40` (u32 LE, payload size) |
//! | 4..8   | `0x3000` (u32 LE, packet type) |
//! | 8..16  | zero |
//! | 16..48 | username `bblp`, NUL-padded |
//! | 48..80 | access code, NUL-padded |
//!
//! The printer then streams frames, each a 16-byte header whose first u32 LE
//! is the JPEG length, followed by that many bytes of JPEG (`FF D8 … FF D9`).
//! Protocol as implemented by the community clients (ha-bambulab
//! `chamber_image.py`, OpenBambuAPI `video.md`); unverified on Cam's
//! firmware until M0.

use std::io::{Read, Write};

use crate::error::{BambuError, Result};

/// TCP port of the P1-series camera stream.
pub const CAMERA_PORT: u16 = 6000;

/// Largest frame we accept. Real frames are ~50–200 kB; anything bigger is a
/// desynchronised stream, not a picture.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Build the 80-byte auth packet.
pub fn auth_packet(access_code: &str) -> Result<[u8; 80]> {
    let code = access_code.as_bytes();
    if code.is_empty() || code.len() > 32 {
        return Err(BambuError::AuthenticationFailed(
            "access code must be 1–32 bytes".into(),
        ));
    }
    let mut p = [0u8; 80];
    p[0..4].copy_from_slice(&0x40u32.to_le_bytes());
    p[4..8].copy_from_slice(&0x3000u32.to_le_bytes());
    p[16..20].copy_from_slice(b"bblp");
    p[48..48 + code.len()].copy_from_slice(code);
    Ok(p)
}

/// Read one frame and return its JPEG bytes, validating the markers.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>> {
    let mut header = [0u8; 16];
    r.read_exact(&mut header).map_err(|e| {
        BambuError::InvalidResponse(format!("camera closed before a frame header: {e}"))
    })?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if !(4..=MAX_FRAME_BYTES).contains(&len) {
        return Err(BambuError::InvalidResponse(format!(
            "camera frame length {len} out of range (auth rejected or stream desynced)"
        )));
    }
    let mut jpeg = vec![0u8; len];
    r.read_exact(&mut jpeg)
        .map_err(|e| BambuError::InvalidResponse(format!("camera frame truncated: {e}")))?;
    if jpeg[..2] != [0xFF, 0xD8] || jpeg[len - 2..] != [0xFF, 0xD9] {
        return Err(BambuError::InvalidResponse(
            "camera frame is not a JPEG (missing SOI/EOI markers)".into(),
        ));
    }
    Ok(jpeg)
}

/// Authenticate on an established (TLS) stream and grab one JPEG.
pub fn snapshot_over<S: Read + Write>(stream: &mut S, access_code: &str) -> Result<Vec<u8>> {
    stream.write_all(&auth_packet(access_code)?)?;
    stream.flush()?;
    read_frame(stream)
}

/// Connect to the printer's camera and grab one JPEG frame.
#[cfg(feature = "network")]
pub fn snapshot(
    ip: std::net::IpAddr,
    access_code: &str,
    timeout: std::time::Duration,
) -> Result<Vec<u8>> {
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::new(ip, CAMERA_PORT);
    let tcp = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| BambuError::ConnectionFailed(format!("camera {addr}: {e}")))?;
    tcp.set_read_timeout(Some(timeout))?;
    tcp.set_write_timeout(Some(timeout))?;
    let name = rustls::pki_types::ServerName::IpAddress(ip.into());
    let conn = rustls::ClientConnection::new(crate::tls::printer_client_config(), name)
        .map_err(|e| BambuError::ConnectionFailed(format!("camera TLS: {e}")))?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    snapshot_over(&mut tls, access_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A frame as the printer would send it.
    pub(crate) fn frame(jpeg: &[u8]) -> Vec<u8> {
        let mut v = (jpeg.len() as u32).to_le_bytes().to_vec();
        v.extend_from_slice(&[0u8; 12]);
        v.extend_from_slice(jpeg);
        v
    }

    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9];

    #[test]
    fn auth_packet_layout() {
        let p = auth_packet("12345678").unwrap();
        assert_eq!(&p[0..4], &[0x40, 0, 0, 0]);
        assert_eq!(&p[4..8], &[0, 0x30, 0, 0]);
        assert_eq!(&p[8..16], &[0; 8]);
        assert_eq!(&p[16..20], b"bblp");
        assert!(p[20..48].iter().all(|&b| b == 0));
        assert_eq!(&p[48..56], b"12345678");
        assert!(p[56..].iter().all(|&b| b == 0));
        assert!(auth_packet("").is_err());
        assert!(auth_packet(&"x".repeat(33)).is_err());
    }

    #[test]
    fn reads_a_frame() {
        assert_eq!(read_frame(&mut Cursor::new(frame(JPEG))).unwrap(), JPEG);
    }

    #[test]
    fn rejects_non_jpeg_truncated_and_oversized() {
        let mut bad = JPEG.to_vec();
        bad[0] = 0;
        assert!(read_frame(&mut Cursor::new(frame(&bad))).is_err());

        let mut short = frame(JPEG);
        short.truncate(short.len() - 2);
        assert!(read_frame(&mut Cursor::new(short)).is_err());

        let mut huge = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
        huge.extend_from_slice(&[0; 12]);
        assert!(read_frame(&mut Cursor::new(huge)).is_err());

        // Auth rejected: printer hangs up without a header.
        assert!(read_frame(&mut Cursor::new(Vec::new())).is_err());
    }

    /// Duplex fake: records writes, serves canned reads.
    struct Duplex {
        rx: Cursor<Vec<u8>>,
        tx: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.rx.read(b)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.tx.write(b)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn snapshot_sends_auth_then_reads_jpeg() {
        let mut s = Duplex {
            rx: Cursor::new(frame(JPEG)),
            tx: Vec::new(),
        };
        assert_eq!(snapshot_over(&mut s, "abc").unwrap(), JPEG);
        assert_eq!(s.tx.len(), 80);
        assert_eq!(&s.tx[48..51], b"abc");
    }
}
