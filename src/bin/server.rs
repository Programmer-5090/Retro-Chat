use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::mpsc,
};
use serde::{Deserialize, Serialize};
use chrono::Local;
use derive_more::Display;
use std::{error::Error, net::SocketAddr, sync::Arc};
use crate::MessageType::SystemNotification;
use sqlx::{Pool, Postgres, postgres::PgPoolOptions};
use redis::Commands;
use argon2::{
    password_hash::{SaltString, PasswordHash, PasswordHasher, PasswordVerifier},
    Argon2,
};
use rand::Rng;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    username: String,
    content: String,
    timestamp: String,
    message_type: MessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize, Display)]
enum MessageType {
    UserMessage,
    SystemNotification,
}

fn build_notice(text: &str) -> String {
    let msg = ChatMessage {
        username: "Server".to_string(),
        content: text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: SystemNotification,
    };
    let json = serde_json::to_string(&msg).unwrap();
    format!("{}\n", json)
}

fn load_tls_config() -> Result<Option<tokio_rustls::TlsAcceptor>, Box<dyn Error>> {
    if std::env::var("NO_TLS").is_ok() {
        return Ok(None);
    }

    let cert_path = std::env::var("TLS_CERT").unwrap_or_else(|_| "cert.pem".to_string());
    let key_path = std::env::var("TLS_KEY").unwrap_or_else(|_| "key.pem".to_string());

    let (cert_der, key_der) = match (
        std::fs::read(&cert_path),
        std::fs::read(&key_path),
    ) {
        (Ok(_), Ok(_)) => {
            let certs: Vec<rustls::pki_types::CertificateDer> = rustls_pemfile::certs(
                &mut std::io::BufReader::new(std::fs::File::open(&cert_path)?),
            )
            .collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile::private_key(
                &mut std::io::BufReader::new(std::fs::File::open(&key_path)?),
            )?
            .ok_or("No private key found in key file")?;
            (certs, key)
        }
        _ => {
            println!("  Generating self-signed certificate...");
            let key_pair = rcgen::KeyPair::generate()?;
            let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])?;
            params
                .distinguished_name
                .push(rcgen::DnType::CommonName, "localhost");
            let cert = params.self_signed(&key_pair)?;
            let cert_der = cert.der().to_vec();
            let key_der = key_pair.serialize_der();
            let certs = vec![rustls::pki_types::CertificateDer::from(cert_der)];
            let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
                rustls::pki_types::PrivatePkcs8KeyDer::from(key_der),
            );
            (certs, key)
        }
    };

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_der, key_der)?;

    Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(config))))
}

async fn handle_connection<S>(
    stream: S,
    _addr: SocketAddr,
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    pool: Pool<Postgres>,
    redis_client: redis::Client,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut redis_conn = redis_client.get_connection().unwrap();

    let mut username = String::new();
    reader.read_line(&mut username).await.unwrap();
    let username = username.trim().to_string();

    if username.len() < 3 || username.len() > 32 || !username.chars().all(|c| c.is_alphanumeric() || c == '_') {
        let notice = build_notice("Username must be 3-32 characters (letters, numbers, underscores).");
        writer.write_all(notice.as_bytes()).await.unwrap();
        return;
    }

    let banned: bool = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM bans WHERE username = $1"
    )
        .bind(&username)
        .fetch_optional(&pool)
        .await
        .unwrap()
        .is_some();
    if banned {
        let notice = build_notice("Your account has been banned.");
        writer.write_all(notice.as_bytes()).await.unwrap();
        return;
    }

    let existing: Result<Option<String>, _> = sqlx::query_scalar(
        "SELECT password_hash FROM users WHERE username = $1"
    )
        .bind(&username)
        .fetch_optional(&pool)
        .await;

    match existing {
        Ok(Some(_)) => {
            let notice = build_notice("Password required. Send: /login <password>");
            writer.write_all(notice.as_bytes()).await.unwrap();
        }
        Ok(None) => {
            let notice = build_notice("Register a password. Send: /register <password>");
            writer.write_all(notice.as_bytes()).await.unwrap();
        }
        Err(_) => {
            let notice = build_notice("Database error.");
            writer.write_all(notice.as_bytes()).await.unwrap();
            return;
        }
    }

    let mut authenticated = false;
    let mut line = String::new();

    while !authenticated {
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        let input = line.trim();

        if let Some(password) = input.strip_prefix("/register ") {
            if password.len() < 8 {
                let notice = build_notice("Password must be at least 8 characters.");
                writer.write_all(notice.as_bytes()).await.unwrap();
                continue;
            }

            let is_first: bool = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM users"
            )
                .fetch_one(&pool)
                .await
                .unwrap_or(0) == 0;
            let role = if is_first { "admin" } else { "user" };

            let salt = SaltString::generate(&mut rand::rngs::OsRng);
            let hash = Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .unwrap()
                .to_string();

            match sqlx::query("INSERT INTO users (username, password_hash, role) VALUES ($1, $2, $3)")
                .bind(&username)
                .bind(&hash)
                .bind(role)
                .execute(&pool)
                .await
            {
                Ok(_) => {
                    let token: String = rand::thread_rng()
                        .sample_iter(&rand::distributions::Alphanumeric)
                        .take(32)
                        .map(char::from)
                        .collect();
                    let _: () = redis_conn
                        .set_ex(format!("session:{}", token), &username, 86400)
                        .unwrap();
                    let notice = build_notice(&format!("Registered and logged in. Token: {}", token));
                    writer.write_all(notice.as_bytes()).await.unwrap();
                    authenticated = true;
                }
                Err(_) => {
                    let notice = build_notice("Username already taken.");
                    writer.write_all(notice.as_bytes()).await.unwrap();
                }
            }
        } else if let Some(password) = input.strip_prefix("/login ") {
            if password.is_empty() {
                let notice = build_notice("Password cannot be empty.");
                writer.write_all(notice.as_bytes()).await.unwrap();
                continue;
            }

            let attempts_key = format!("login_attempts:{}", username);
            let attempts: i32 = redis_conn.get(&attempts_key).unwrap_or(0);
            if attempts >= 3 {
                let ttl: i64 = redis_conn.ttl(&attempts_key).unwrap_or(60);
                let notice = build_notice(&format!("Too many failed attempts. Try again in {} seconds.", ttl));
                writer.write_all(notice.as_bytes()).await.unwrap();
                continue;
            }

            match sqlx::query_scalar::<_, String>(
                "SELECT password_hash FROM users WHERE username = $1"
            )
                .bind(&username)
                .fetch_optional(&pool)
                .await
            {
                Ok(Some(stored_hash)) => {
                    let parsed = PasswordHash::new(&stored_hash).unwrap();
                    if Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok() {
                        let _: () = redis_conn.del(&attempts_key).unwrap();
                        let token: String = rand::thread_rng()
                            .sample_iter(&rand::distributions::Alphanumeric)
                            .take(32)
                            .map(char::from)
                            .collect();
                        let _: () = redis_conn
                            .set_ex(format!("session:{}", token), &username, 86400)
                            .unwrap();
                            let notice = build_notice(&format!("Logged in. Token: {}", token));
                            writer.write_all(notice.as_bytes()).await.unwrap();
                        authenticated = true;
                    } else {
                        let count: i32 = redis_conn.incr(&attempts_key, 1).unwrap();
                        if count == 1 {
                            let _: () = redis_conn.expire(&attempts_key, 60).unwrap();
                        }
                        let notice = build_notice(&format!("Wrong password. ({}/3 attempts)", count));
                        writer.write_all(notice.as_bytes()).await.unwrap();
                    }
                }
                Ok(None) => {
                    let notice = build_notice("User not found. Use /register first.");
                    writer.write_all(notice.as_bytes()).await.unwrap();
                }
                Err(_) => {
                    let notice = build_notice("Database error.");
                    writer.write_all(notice.as_bytes()).await.unwrap();
                    return;
                }
            }
        } else {
            let notice = build_notice("Authenticate first: /register <password> or /login <password>");
            writer.write_all(notice.as_bytes()).await.unwrap();
        }
    }

    let _: () = redis_conn
        .set_ex(format!("presence:{}", username), "online", 30)
        .unwrap();

    let user_role: String = sqlx::query_scalar(
        "SELECT role FROM users WHERE username = $1"
    )
        .bind(&username)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|_| "user".to_string());

    let rows = sqlx::query_as::
        <_, (String, String, chrono::DateTime<chrono::Utc>, String)>
        ("SELECT username, content, created_at, message_type FROM messages ORDER BY created_at DESC LIMIT 50")
        .fetch_all(&pool)
        .await.unwrap();

    for row in rows.into_iter().rev() {
        let msg = ChatMessage {
            username: row.0,
            content: row.1,
            timestamp: row.2.format("%H:%M:%S").to_string(),
            message_type: match row.3.as_str() {
                "UserMessage" => MessageType::UserMessage,
                _ => MessageType::SystemNotification,
            }
        };
        let msg_json = serde_json::to_string(&msg).unwrap();
        writer.write_all(msg_json.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
    }

    line.clear();

    let join_msg = ChatMessage {
        username: username.clone(),
        content: "Joined the chat".to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: SystemNotification,
    };
    let join_json = serde_json::to_string(&join_msg).unwrap();
    tx.send(join_json).unwrap();

    loop {
        tokio::select! {
            result = reader.read_line(&mut line) => {
                if result.unwrap() == 0 {
                    break;
                }

                let input = line.trim();

                if input.starts_with('/') {
                    if let Some(target) = input.strip_prefix("/mute ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /mute.");
                            writer.write_all(notice.as_bytes()).await.unwrap();
                        } else {
                            let parts: Vec<&str> = target.splitn(2, ' ').collect();
                            if parts.len() < 2 {
                                let notice = build_notice("Usage: /mute <username> <minutes>");
                                writer.write_all(notice.as_bytes()).await.unwrap();
                            } else {
                                let target_user = parts[0].trim();
                                let minutes: u64 = parts[1].trim().parse().unwrap_or(5);
                                let seconds = minutes * 60;
                                let _: () = redis_conn
                                    .set_ex(format!("muted:{}", target_user), "1", seconds)
                                    .unwrap();
                                let notice = build_notice(&format!("Muted {} for {} minutes.", target_user, minutes));
                                writer.write_all(notice.as_bytes()).await.unwrap();
                                sqlx::query("INSERT INTO audit_log (actor, action, target, details) VALUES ($1, 'mute', $2, $3)")
                                    .bind(&username)
                                    .bind(target_user)
                                    .bind(format!("{} minutes", minutes))
                                    .execute(&pool)
                                    .await
                                    .unwrap();
                            }
                        }
                    } else if let Some(target) = input.strip_prefix("/unmute ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /unmute.");
                            writer.write_all(notice.as_bytes()).await.unwrap();
                        } else {
                            let target_user = target.trim();
                            let _: () = redis_conn.del(format!("muted:{}", target_user)).unwrap();
                            let notice = build_notice(&format!("Unmuted {}.", target_user));
                            writer.write_all(notice.as_bytes()).await.unwrap();
                            sqlx::query("INSERT INTO audit_log (actor, action, target) VALUES ($1, 'unmute', $2)")
                                .bind(&username)
                                .bind(target_user)
                                .execute(&pool)
                                .await
                                .unwrap();
                        }
                    } else if let Some(target) = input.strip_prefix("/ban ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /ban.");
                            writer.write_all(notice.as_bytes()).await.unwrap();
                        } else {
                            let parts: Vec<&str> = target.splitn(2, ' ').collect();
                            let target_user = parts[0].trim();
                            let reason = if parts.len() > 1 { parts[1].trim() } else { "No reason" };
                            match sqlx::query("INSERT INTO bans (username, banned_by, reason) VALUES ($1, $2, $3)")
                                .bind(target_user)
                                .bind(&username)
                                .bind(reason)
                                .execute(&pool)
                                .await
                            {
                                Ok(_) => {
                                    let notice = build_notice(&format!("Banned {} (reason: {}).", target_user, reason));
                                    writer.write_all(notice.as_bytes()).await.unwrap();
                                    sqlx::query("INSERT INTO audit_log (actor, action, target, details) VALUES ($1, 'ban', $2, $3)")
                                        .bind(&username)
                                        .bind(target_user)
                                        .bind(reason)
                                        .execute(&pool)
                                        .await
                                        .unwrap();
                                }
                                Err(_) => {
                                    let notice = build_notice(&format!("User '{}' is not registered.", target_user));
                                    writer.write_all(notice.as_bytes()).await.unwrap();
                                }
                            }
                        }
                    } else if let Some(target) = input.strip_prefix("/unban ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /unban.");
                            writer.write_all(notice.as_bytes()).await.unwrap();
                        } else {
                            let target_user = target.trim();
                            sqlx::query("DELETE FROM bans WHERE username = $1")
                                .bind(target_user)
                                .execute(&pool)
                                .await
                                .unwrap();
                            let notice = build_notice(&format!("Unbanned {}.", target_user));
                            writer.write_all(notice.as_bytes()).await.unwrap();
                            sqlx::query("INSERT INTO audit_log (actor, action, target) VALUES ($1, 'unban', $2)")
                                .bind(&username)
                                .bind(target_user)
                                .execute(&pool)
                                .await
                                .unwrap();
                        }
                    } else {
                        let notice = build_notice("Unknown command. Available: /mute, /unmute, /ban, /unban");
                        writer.write_all(notice.as_bytes()).await.unwrap();
                    }
                    line.clear();
                    continue;
                }

                if input.len() > 4096 {
                    let notice = build_notice("Message too long (max 4096 characters).");
                    writer.write_all(notice.as_bytes()).await.unwrap();
                    line.clear();
                    continue;
                }

                let muted: Option<String> = redis_conn.get(format!("muted:{}", username)).ok();
                if muted.is_some() {
                    let notice = build_notice("You are muted and cannot send messages.");
                    writer.write_all(notice.as_bytes()).await.unwrap();
                    line.clear();
                    continue;
                }

                let msg = ChatMessage {
                    username: username.clone(),
                    content: input.to_string(),
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    message_type: MessageType::UserMessage,
                };

                let json = serde_json::to_string(&msg).unwrap();

                let count: i32 = redis_conn
                    .incr(format!("ratelimit:{}", msg.username), 1)
                    .unwrap();
                if count == 1 {
                    let _: () = redis_conn
                        .expire(format!("ratelimit:{}", msg.username), 10)
                        .unwrap();
                }
                if count > 20 {
                    let warn = ChatMessage {
                        username: "Server".to_string(),
                        content: "Rate limit exceeded. Slow down.".to_string(),
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        message_type: SystemNotification,
                    };
                    let warn_json = serde_json::to_string(&warn).unwrap();
                    writer.write_all(warn_json.as_bytes()).await.unwrap();
                    writer.write_all(b"\n").await.unwrap();
                    line.clear();
                    continue;
                }

                tx.send(json).unwrap();
                sqlx::query("INSERT INTO messages (username, content, message_type) VALUES ($1, $2, $3)")
                    .bind(&msg.username)
                    .bind(&msg.content)
                    .bind((msg.message_type).to_string())
                    .execute(&pool)
                    .await
                    .unwrap();

                line.clear();
            }

            result = rx.recv() => {
                let msg = result.unwrap();
                writer.write_all(msg.as_bytes()).await.unwrap();
                writer.write_all(b"\n").await.unwrap();
            }
        }
    }

    let _: () = redis_conn.del(format!("presence:{}", username)).unwrap();

    let leave_msg = ChatMessage {
        username: username.clone(),
        content: "Left the chat".to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: SystemNotification,
    };
    let leave_json = serde_json::to_string(&leave_msg).unwrap();
    tx.send(leave_json).unwrap();

    println!(
        "└─[{}] {} disconnected",
        Local::now().format("%H:%M:%S"),
        username
    );
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8082".to_string());
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());

    let listener = TcpListener::bind(&bind_addr).await?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&std::env::var("DATABASE_URL")?)
        .await?;
    let redis_client = redis::Client::open(redis_url.as_str())?;

    let tls_acceptor = load_tls_config()?;

    sqlx::migrate!().run(&pool).await?;

    sqlx::query("INSERT INTO rooms (name, created_by) VALUES ('general', 'system') ON CONFLICT (name) DO NOTHING")
        .execute(&pool)
        .await?;

    let tls_status = if tls_acceptor.is_some() { "TLS" } else { "TCP" };
    println!("╔═════════════════════════════════════════════╗");
    println!("║          RETRO CHAT SERVER ACTIVE           ║");
    println!("║   {tls_status}  Listening on: {bind_addr}   ║");
    println!("║          Press Ctrl+C to shutdown           ║");
    println!("╚═════════════════════════════════════════════╝");

    loop {
        let (socket, addr) = listener.accept().await?;

        println!("┌─[{}] New connection", Local::now().format("%H:%M:%S"));
        println!("└─ Address: {}", addr);

        let pool = pool.clone();
        let redis_client = redis_client.clone();

        let ip = addr.ip();
        let conn_count: i32 = redis_client
            .get_connection()
            .unwrap()
            .incr(format!("conn_ratelimit:{}", ip), 1)
            .unwrap();
        if conn_count == 1 {
            let _: () = redis_client
                .get_connection()
                .unwrap()
                .expire(format!("conn_ratelimit:{}", ip), 60)
                .unwrap();
        }
        if conn_count > 20 {
            println!("  └─ Connection rate limited (IP: {})", ip);
            continue;
        }

        if let Some(ref acceptor) = tls_acceptor {
            match acceptor.accept(socket).await {
                Ok(tls_stream) => {
                    tokio::spawn(async move {
                        handle_connection(tls_stream, addr, pool, redis_client).await
                    });
                }
                Err(e) => {
                    println!("  └─ TLS handshake failed: {e}");
                }
            }
        } else {
            tokio::spawn(async move {
                handle_connection(socket, addr, pool, redis_client).await
            });
        }
    }
}