//! FTPS upload for Bambu Lab printers.
//!
//! Bambu printers accept 3MF files via FTPS on port 990 (implicit TLS).
//! The printer uses a self-signed certificate, so we must
//! accept invalid certs.

use std::io::Cursor;
use std::net::IpAddr;
use std::sync::Arc;

use rustls::ClientConfig;
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

    tokio::task::spawn_blocking(move || upload_3mf_sync(ip, &access_code, &filename, &data))
        .await
        .map_err(|e| BambuError::PrintError(format!("FTPS task panicked: {}", e)))?
}

/// Synchronous FTPS upload implementation.
fn upload_3mf_sync(ip: IpAddr, access_code: &str, filename: &str, data: &[u8]) -> Result<()> {
    let addr = format!("{}:990", ip);

    // Build rustls config that accepts self-signed certs
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAllCerts))
        .with_no_client_auth();
    let connector: RustlsConnector = Arc::new(config).into();

    let mut ftp = RustlsFtpStream::connect_secure_implicit(&addr, connector, &ip.to_string())
        .map_err(|e| BambuError::ConnectionFailed(format!("FTPS connect failed: {}", e)))?;

    ftp.login("bblp", access_code)
        .map_err(|e| BambuError::AuthenticationFailed(format!("FTPS login failed: {}", e)))?;

    let mut reader = Cursor::new(data);
    ftp.put_file(filename, &mut reader)
        .map_err(|e| BambuError::PrintError(format!("FTPS upload failed: {}", e)))?;

    ftp.quit()
        .map_err(|e| BambuError::PrintError(format!("FTPS quit failed: {}", e)))?;

    Ok(())
}

/// Certificate verifier that accepts all certificates (for self-signed Bambu certs).
#[derive(Debug)]
struct AcceptAllCerts;

impl rustls::client::danger::ServerCertVerifier for AcceptAllCerts {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}
