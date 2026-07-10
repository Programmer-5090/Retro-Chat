use std::sync::Arc;
use std::error::Error;

pub fn load_tls_config() -> Result<Option<tokio_rustls::TlsAcceptor>, Box<dyn Error>> {
    if std::env::var("NO_TLS").is_ok() {
        return Ok(None);
    }

    let cert_path = std::env::var("TLS_CERT").unwrap_or_else(|_| "cert.pem".to_string());
    let key_path = std::env::var("TLS_KEY").unwrap_or_else(|_| "key.pem".to_string());

    let (cert_der, key_der) = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(_), Ok(_)) => {
            let certs: Vec<rustls::pki_types::CertificateDer> = rustls_pemfile
                ::certs(&mut std::io::BufReader::new(std::fs::File::open(&cert_path)?))
                .collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile
                ::private_key(&mut std::io::BufReader::new(std::fs::File::open(&key_path)?))?
                .ok_or("No private key found in key file")?;
            (certs, key)
        }
        _ => {
            println!("  Generating self-signed certificate...");
            let key_pair = rcgen::KeyPair::generate()?;
            let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])?;
            params.distinguished_name.push(rcgen::DnType::CommonName, "localhost");
            let cert = params.self_signed(&key_pair)?;
            let cert_der = cert.der().to_vec();
            let key_der = key_pair.serialize_der();
            let certs = vec![rustls::pki_types::CertificateDer::from(cert_der)];
            let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
                rustls::pki_types::PrivatePkcs8KeyDer::from(key_der)
            );
            (certs, key)
        }
    };

    let config = rustls::ServerConfig
        ::builder()
        .with_no_client_auth()
        .with_single_cert(cert_der, key_der)?;

    Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(config))))
}
