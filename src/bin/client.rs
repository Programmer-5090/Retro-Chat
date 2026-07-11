use std::{ env, error::Error };
use tokio::{ io::AsyncWriteExt, net::TcpStream };
use crossterm::terminal::disable_raw_mode;

use retro_chat::client_helpers::{ ClientStream, create_tls_connector };
use retro_chat::tui::{ run_chat_ui, run_login_ui };

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = disable_raw_mode();

    let username = env::args().nth(1).expect("Please provide a username as argument");

    let no_tls = std::env::var("NO_TLS").is_ok() || env::args().any(|a| a == "--no-tls");

    let stream = if no_tls {
        ClientStream::Plain(TcpStream::connect("127.0.0.1:8082").await?)
    } else {
        let tcp = TcpStream::connect("127.0.0.1:8082").await?;
        let connector = create_tls_connector()?;
        let domain = rustls::pki_types::ServerName::try_from("localhost")?;
        match connector.connect(domain, tcp).await {
            Ok(tls) => ClientStream::Tls(tokio_rustls::TlsStream::Client(tls)),
            Err(e) => {
                eprintln!("TLS connection failed ({}), falling back to plain TCP", e);
                ClientStream::Plain(TcpStream::connect("127.0.0.1:8082").await?)
            }
        }
    };

    let (reader, mut writer) = tokio::io::split(stream);

    writer.write_all(format!("{}\n", username).as_bytes()).await?;

    let reader = tokio::io::BufReader::new(reader);

    // Shows the ByteChat splash screen, then collects the register/login
    // password inside the same terminal UI before handing off to the chat.
    let (reader, writer) = run_login_ui(reader, writer).await?;

    run_chat_ui(username, reader, writer).await
}