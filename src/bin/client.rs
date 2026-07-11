use std::{ env, error::Error, io::{ self, Write } };
use tokio::{ io::{ AsyncBufReadExt, AsyncWriteExt, BufReader }, net::TcpStream };
use crossterm::terminal::disable_raw_mode;

use retro_chat::client_helpers::{ ClientStream, create_tls_connector };
use retro_chat::tui::run_chat_ui;
use retro_chat::ChatMessage;

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

    let mut reader = BufReader::new(reader);
    let mut buf = String::new();

    loop {
        buf.clear();
        reader.read_line(&mut buf).await?;
        if buf.is_empty() {
            break;
        }
        let msg = match serde_json::from_str::<ChatMessage>(buf.trim()) {
            Ok(m) => m,
            Err(_) => {
                continue;
            }
        };
        if msg.username != "Server" {
            continue;
        }
        println!("{}", msg.content);
        if msg.content.contains("Token:") {
            break;
        }
        print!("Enter password: ");
        io::stdout().flush()?;
        let mut password = String::new();
        io::stdin().read_line(&mut password)?;
        let password = password.trim().to_string();
        let password = password
            .strip_prefix("/register ")
            .or(password.strip_prefix("/login "))
            .unwrap_or(&password)
            .to_string();
        let cmd = if msg.content.contains("Register") {
            format!("/register {}\n", password)
        } else {
            format!("/login {}\n", password)
        };
        writer.write_all(cmd.as_bytes()).await?;
    }

    run_chat_ui(username, reader, writer).await
}
