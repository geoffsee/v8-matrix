use std::sync::Arc;

use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::ResolvesServerCert;
use sha2::{Digest, Sha256};

pub enum TlsMode {
    SelfSigned {
        rustls_config: Arc<rustls::ServerConfig>,
        spki_hash_b64: String,
    },
    Acme {
        acme_state: rustls_acme::AcmeState<std::io::Error>,
    },
}

impl TlsMode {
    /// Resolver for sharing with quinn. In ACME mode this auto-updates on renewal.
    pub fn quinn_rustls_config(&self) -> Arc<rustls::ServerConfig> {
        let resolver: Arc<dyn ResolvesServerCert> = match self {
            TlsMode::SelfSigned { rustls_config, .. } => rustls_config.cert_resolver.clone(),
            TlsMode::Acme { acme_state, .. } => acme_state.resolver().clone(),
        };
        let mut cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver);
        cfg.alpn_protocols = vec![b"h3".to_vec()];
        cfg.max_early_data_size = u32::MAX;
        Arc::new(cfg)
    }
}

pub fn generate_self_signed() -> TlsMode {
    let CertifiedKey { cert, key_pair } = generate_simple_self_signed(vec![
        "localhost".into(),
        "127.0.0.1".into(),
    ])
    .expect("generate self-signed cert");

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    let spki_hash_b64 = {
        use base64::Engine;
        let hash = Sha256::digest(key_pair.public_key_der());
        base64::engine::general_purpose::STANDARD.encode(hash)
    };

    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("build rustls config");
    cfg.alpn_protocols = vec![b"h3".to_vec()];
    cfg.max_early_data_size = u32::MAX;

    TlsMode::SelfSigned {
        rustls_config: Arc::new(cfg),
        spki_hash_b64,
    }
}

pub fn acme(domain: &str) -> TlsMode {
    use rustls_acme::caches::DirCache;

    let state = rustls_acme::AcmeConfig::new([domain])
        .contact_push(format!("mailto:acme@{domain}"))
        .cache(DirCache::new("/data/acme_cache"))
        .directory_lets_encrypt(true)
        .state();

    TlsMode::Acme { acme_state: state }
}
