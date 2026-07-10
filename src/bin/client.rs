use cursive::{
    Cursive,
    align::HAlign,
    event::Key,
    theme::{ BaseColor, BorderStyle, Color, Palette, PaletteColor, Theme },
    traits::*,
    views::{ Dialog, DummyView, EditView, LinearLayout, Panel, ScrollView, TextView },
};

use chrono::Local;
use serde::{ Deserialize, Serialize };
use std::{ env, error::Error, io::{ self, Write }, pin::Pin, sync::Arc, task::{ Context, Poll } };
use tokio::{
    io::{ AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf },
    net::TcpStream,
    sync::Mutex,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    username: String,
    content: String,
    timestamp: String,
    message_type: MessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum MessageType {
    UserMessage,
    SystemNotification,
}

enum ClientStream {
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

type IoWriter = tokio::io::WriteHalf<ClientStream>;

fn send_message(siv: &mut Cursive, msg: String) {
    if msg.is_empty() {
        return;
    }

    match msg.as_str() {
        "/help" => {
            siv.call_on_name("messages", |view: &mut TextView| {
                view.append(
                    "\n=== Commands ===\n/help - Show this help\n/clear - Clear messages\n/quit - Exit chat\n\n"
                );
            });
            siv.call_on_name("input", |view: &mut EditView| {
                view.set_content("");
            });
            return;
        }
        "/clear" => {
            siv.call_on_name("messages", |view: &mut TextView| {
                view.set_content("");
            });
            siv.call_on_name("input", |view: &mut EditView| {
                view.set_content("");
            });
            return;
        }
        "/quit" => {
            siv.quit();
            return;
        }
        _ => {}
    }

    let writer = siv.user_data::<Arc<Mutex<IoWriter>>>().unwrap().clone();

    tokio::spawn(async move {
        let _ = writer.lock().await.write_all(format!("{}\n", msg).as_bytes()).await;
    });

    siv.call_on_name("input", |view: &mut EditView| {
        view.set_content("");
    });
}

fn create_retro_theme() -> Theme {
    let mut palette = Palette::default();
    palette[PaletteColor::Background] = Color::Dark(BaseColor::Black);
    palette[PaletteColor::View] = Color::Dark(BaseColor::Black);
    palette[PaletteColor::Primary] = Color::Light(BaseColor::Green);
    palette[PaletteColor::Secondary] = Color::Light(BaseColor::Green);
    palette[PaletteColor::Tertiary] = Color::Light(BaseColor::White);
    palette[PaletteColor::TitlePrimary] = Color::Light(BaseColor::Green);
    palette[PaletteColor::Highlight] = Color::Light(BaseColor::Green);

    Theme {
        palette,
        borders: BorderStyle::Simple,
        ..Default::default()
    }
}

fn create_tls_connector() -> Result<tokio_rustls::TlsConnector, Box<dyn Error>> {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
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
        if
            let Ok(msg) = serde_json::from_str::<ChatMessage>(buf.trim()) &&
            msg.username == "Server"
        {
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
    }

    let mut siv = cursive::default();
    siv.set_theme(create_retro_theme());

    let header = TextView::new(
        format!(r#"╔═ RETRO CHAT ═╗ User: {} ╔═ {} ═╗"#, username, Local::now().format("%H:%M:%S"))
    )
        .style(Color::Light(BaseColor::Green))
        .h_align(HAlign::Center);

    let messages = TextView::new("").with_name("messages").min_height(20).scrollable();

    let messages = ScrollView::new(messages)
        .scroll_strategy(cursive::view::ScrollStrategy::StickToBottom)
        .min_width(60)
        .full_width();

    let input = EditView::new()
        .on_submit(move |s, text| send_message(s, text.to_string()))
        .with_name("input")
        .min_width(50)
        .max_height(3)
        .full_width();

    let help_text = TextView::new("ESC:quit | Enter:send | Commands: /help, /clear, /quit").style(
        Color::Dark(BaseColor::White)
    );

    let layout = LinearLayout::vertical()
        .child(Panel::new(header))
        .child(
            Dialog::around(messages).title("Messages").title_position(HAlign::Center).full_width()
        )
        .child(Dialog::around(input).title("Message").title_position(HAlign::Center).full_width())
        .child(Panel::new(help_text).full_width());

    let centered_layout = LinearLayout::horizontal()
        .child(DummyView.full_width())
        .child(layout)
        .child(DummyView.full_width());

    siv.add_fullscreen_layer(centered_layout);
    siv.add_global_callback(Key::Esc, |s| s.quit());
    siv.add_global_callback('/', |s| {
        s.call_on_name("input", |view: &mut EditView| {
            view.set_content("/");
        });
    });

    let writer = Arc::new(Mutex::new(writer));
    let writer_clone = Arc::clone(&writer);
    siv.set_user_data(writer);

    let mut lines = reader.lines();
    let sink = siv.cb_sink().clone();

    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(msg) = serde_json::from_str::<ChatMessage>(&line) {
                let formatted_msg = match msg.message_type {
                    MessageType::UserMessage => {
                        format!("┌─[{}]\n└─ {} ▶ {}\n", msg.timestamp, msg.username, msg.content)
                    }
                    MessageType::SystemNotification => {
                        format!("\n[{} {}]\n", msg.username, msg.content)
                    }
                };

                if
                    sink
                        .send(
                            Box::new(move |siv: &mut Cursive| {
                                siv.call_on_name("messages", |view: &mut TextView| {
                                    view.append(formatted_msg);
                                });
                            })
                        )
                        .is_err()
                {
                    break;
                }
            }
        }
    });

    siv.run();
    let _ = writer_clone.lock().await.shutdown().await;
    Ok(())
}
