use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, DnType};
use std::fs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let ca_path = std::env::var("CA_CERT").unwrap_or_else(|_| "ca.pem".to_string());
    let ca_key_path = std::env::var("CA_KEY").unwrap_or_else(|_| "ca-key.pem".to_string());
    let cert_path = std::env::var("TLS_CERT").unwrap_or_else(|_| "cert.pem".to_string());
    let key_path = std::env::var("TLS_KEY").unwrap_or_else(|_| "key.pem".to_string());

    eprintln!("Generating CA...");
    let ca_key = KeyPair::generate()?;
    let ca_key_pem = ca_key.serialize_pem();
    let mut ca_params = CertificateParams::new(vec![])?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "RetroChat CA");
    let ca_cert = ca_params.self_signed(&ca_key)?;

    eprintln!("Generating server certificate...");
    let server_key = KeyPair::generate()?;
    let server_key_pem = server_key.serialize_pem();
    let mut server_params = CertificateParams::new(vec!["localhost".to_string()])?;
    server_params.distinguished_name.push(DnType::CommonName, "localhost");
    let issuer = Issuer::new(ca_params, ca_key);
    let server_cert = server_params.signed_by(&server_key, &issuer)?;

    fs::write(&ca_path, ca_cert.pem().as_bytes())?;
    eprintln!("Wrote {}", ca_path);
    fs::write(&ca_key_path, ca_key_pem.as_bytes())?;
    eprintln!("Wrote {}", ca_key_path);
    fs::write(&cert_path, server_cert.pem().as_bytes())?;
    eprintln!("Wrote {}", cert_path);
    fs::write(&key_path, server_key_pem.as_bytes())?;
    eprintln!("Wrote {}", key_path);

    Ok(())
}
