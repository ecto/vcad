//! TLS for the printer's LAN services.
//!
//! Every LAN service on a Bambu printer (MQTT 8883, FTPS 990, P1 camera
//! 6000) presents a certificate signed by Bambu's own per-device CA, not a
//! public root, so standard verification always fails. These configs accept
//! the printer's certificate; the access code is the actual credential.
//! Only ever point them at an address the user configured or discovered on
//! their own LAN.

use std::sync::Arc;

/// Implement an accept-everything verifier for one rustls major version.
/// rumqttc 0.24 pins rustls 0.22 while suppaftp uses 0.23, so the same
/// verifier exists twice.
macro_rules! accept_all_verifier {
    ($name:ident, $rustls:ident) => {
        #[derive(Debug)]
        pub(crate) struct $name;

        impl $rustls::client::danger::ServerCertVerifier for $name {
            fn verify_server_cert(
                &self,
                _end_entity: &$rustls::pki_types::CertificateDer<'_>,
                _intermediates: &[$rustls::pki_types::CertificateDer<'_>],
                _server_name: &$rustls::pki_types::ServerName<'_>,
                _ocsp_response: &[u8],
                _now: $rustls::pki_types::UnixTime,
            ) -> std::result::Result<$rustls::client::danger::ServerCertVerified, $rustls::Error>
            {
                Ok($rustls::client::danger::ServerCertVerified::assertion())
            }

            fn verify_tls12_signature(
                &self,
                _message: &[u8],
                _cert: &$rustls::pki_types::CertificateDer<'_>,
                _dss: &$rustls::DigitallySignedStruct,
            ) -> std::result::Result<
                $rustls::client::danger::HandshakeSignatureValid,
                $rustls::Error,
            > {
                Ok($rustls::client::danger::HandshakeSignatureValid::assertion())
            }

            fn verify_tls13_signature(
                &self,
                _message: &[u8],
                _cert: &$rustls::pki_types::CertificateDer<'_>,
                _dss: &$rustls::DigitallySignedStruct,
            ) -> std::result::Result<
                $rustls::client::danger::HandshakeSignatureValid,
                $rustls::Error,
            > {
                Ok($rustls::client::danger::HandshakeSignatureValid::assertion())
            }

            fn supported_verify_schemes(&self) -> Vec<$rustls::SignatureScheme> {
                use $rustls::SignatureScheme as S;
                vec![
                    S::RSA_PKCS1_SHA256,
                    S::RSA_PKCS1_SHA384,
                    S::RSA_PKCS1_SHA512,
                    S::ECDSA_NISTP256_SHA256,
                    S::ECDSA_NISTP384_SHA384,
                    S::ECDSA_NISTP521_SHA512,
                    S::RSA_PSS_SHA256,
                    S::RSA_PSS_SHA384,
                    S::RSA_PSS_SHA512,
                    S::ED25519,
                    S::ED448,
                ]
            }
        }
    };
}

accept_all_verifier!(AcceptPrinterCert, rustls);
accept_all_verifier!(AcceptPrinterCertMqtt, rustls022);

/// rustls 0.23 client config for FTPS and the camera stream.
pub(crate) fn printer_client_config() -> Arc<rustls::ClientConfig> {
    Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
            .with_safe_default_protocol_versions()
            .expect("ring provider supports the default TLS protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptPrinterCert))
            .with_no_client_auth(),
    )
}

/// rustls 0.22 client config for MQTT (rumqttc's TLS stack).
pub(crate) fn mqtt_client_config() -> Arc<rustls022::ClientConfig> {
    Arc::new(
        rustls022::ClientConfig::builder_with_provider(Arc::new(
            rustls022::crypto::ring::default_provider(),
        ))
            .with_safe_default_protocol_versions()
            .expect("ring provider supports the default TLS protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptPrinterCertMqtt))
            .with_no_client_auth(),
    )
}
