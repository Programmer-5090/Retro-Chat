use std::{io, pin::Pin, sync::Arc, task::{Context, Poll}};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

pub enum ClientStream {
    Plain(TcpStream),
    Tls(tokio_rustls::TlsStream<TcpStream>),
}

impl Unpin for ClientStream {}

impl AsyncRead for ClientStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            ClientStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ClientStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8]
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            ClientStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_flush(cx),
            ClientStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ClientStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            ClientStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

pub type IoWriter = tokio::io::WriteHalf<ClientStream>;

pub fn create_tls_connector() -> Result<tokio_rustls::TlsConnector, Box<dyn std::error::Error>> {
    let ca_path = std::env::var("CA_CERT").unwrap_or_else(|_| "ca.pem".to_string());

    let ca_pem = match std::fs::read(&ca_path) {
        Ok(data) => data,
        Err(_) => {
            eprintln!("CA cert not found at '{}'. Run `cargo run --bin init-tls` first, or set CA_CERT env var.", ca_path);
            return Err("CA cert not found".into());
        }
    };

    let mut root_store = rustls::RootCertStore::empty();
    let certs = rustls_pemfile
        ::certs(&mut std::io::BufReader::new(ca_pem.as_slice()))
        .collect::<Result<Vec<_>, _>>()?;
    for cert in certs {
        root_store.add(cert)?;
    }

    let config = rustls::ClientConfig
        ::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}
