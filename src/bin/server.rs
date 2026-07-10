use std::collections::HashMap;
use std::sync::Arc;

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::{mpsc, RwLock},
};
use serde::{Deserialize, Serialize};
use chrono::Local;
use derive_more::Display;
use std::{error::Error, net::SocketAddr};
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

struct AppState {
    rooms: RwLock<HashMap<String, Vec<(String, mpsc::UnboundedSender<String>)>>>,
    user_rooms: RwLock<HashMap<String, String>>,
    user_senders: RwLock<HashMap<String, mpsc::UnboundedSender<String>>>,
    pool: Pool<Postgres>,
    redis_client: redis::Client,
}

impl AppState {
    fn new(pool: Pool<Postgres>, redis_client: redis::Client) -> Arc<Self> {
        Arc::new(Self {
            rooms: RwLock::new(HashMap::new()),
            user_rooms: RwLock::new(HashMap::new()),
            user_senders: RwLock::new(HashMap::new()),
            pool,
            redis_client,
        })
    }

    async fn join_room(
        &self,
        username: &str,
        room: &str,
        tx: mpsc::UnboundedSender<String>,
    ) {
        let mut rooms = self.rooms.write().await;
        for (_, members) in rooms.iter_mut() {
            members.retain(|(name, _)| name != username);
        }
        rooms
            .entry(room.to_string())
            .or_default()
            .push((username.to_string(), tx));
        drop(rooms);

        self.user_rooms
            .write()
            .await
            .insert(username.to_string(), room.to_string());
    }

    async fn leave_all_rooms(&self, username: &str) {
        let mut rooms = self.rooms.write().await;
        for (_, members) in rooms.iter_mut() {
            members.retain(|(name, _)| name != username);
        }
    }

    async fn send_to_room(&self, room: &str, msg: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(members) = rooms.get_mut(room) {
            members.retain(|(_, tx)| tx.send(msg.to_string()).is_ok());
        }
    }

    async fn get_user_room(&self, username: &str) -> String {
        self.user_rooms
            .read()
            .await
            .get(username)
            .cloned()
            .unwrap_or_else(|| "general".to_string())
    }

    async fn get_or_create_db_room(&self, room: &str, created_by: &str) -> i32 {
        sqlx::query(
            "INSERT INTO rooms (name, created_by) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING",
        )
        .bind(room)
        .bind(created_by)
        .execute(&self.pool)
        .await
        .ok();

        sqlx::query_scalar::<_, i32>("SELECT id FROM rooms WHERE name = $1")
            .bind(room)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(1)
    }

    async fn list_db_rooms(&self) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM rooms WHERE name NOT LIKE '__dm\\_\\_%' ORDER BY name",
        )
        .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
    }

    async fn register_sender(&self, username: &str, tx: mpsc::UnboundedSender<String>) {
        self.user_senders
            .write()
            .await
            .insert(username.to_string(), tx);
    }

    async fn unregister_sender(&self, username: &str) {
        self.user_senders.write().await.remove(username);
    }

    async fn get_sender(&self, username: &str) -> Option<mpsc::UnboundedSender<String>> {
        self.user_senders.read().await.get(username).cloned()
    }
}

async fn handle_connection<S>(
    stream: S,
    _addr: SocketAddr,
    state: Arc<AppState>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut redis_conn = state.redis_client.get_connection().unwrap();

    let mut username = String::new();
    if reader.read_line(&mut username).await.unwrap_or(0) == 0 {
        return;
    }
    let username = username.trim().to_string();

    if username.len() < 3 || username.len() > 32 || !username.chars().all(|c| c.is_alphanumeric() || c == '_') {
        let notice = build_notice("Username must be 3-32 characters (letters, numbers, underscores).");
        let _ = writer.write_all(notice.as_bytes()).await;
        return;
    }

    let banned: bool = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM bans WHERE username = $1"
    )
        .bind(&username)
        .fetch_optional(&state.pool)
        .await
        .unwrap()
        .is_some();
    if banned {
        let notice = build_notice("Your account has been banned.");
        let _ = writer.write_all(notice.as_bytes()).await;
        return;
    }

    let existing: Result<Option<String>, _> = sqlx::query_scalar(
        "SELECT password_hash FROM users WHERE username = $1"
    )
        .bind(&username)
        .fetch_optional(&state.pool)
        .await;

    match existing {
        Ok(Some(_)) => {
            let notice = build_notice("Password required. Send: /login <password>");
            let _ = writer.write_all(notice.as_bytes()).await;
        }
        Ok(None) => {
            let notice = build_notice("Register a password. Send: /register <password>");
            let _ = writer.write_all(notice.as_bytes()).await;
        }
        Err(_) => {
            let notice = build_notice("Database error.");
            let _ = writer.write_all(notice.as_bytes()).await;
            return;
        }
    }

    let mut authenticated = false;
    let mut line = String::new();

    while !authenticated {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let input = line.trim();

        if let Some(password) = input.strip_prefix("/register ") {
            if password.len() < 8 {
                let notice = build_notice("Password must be at least 8 characters.");
                let _ = writer.write_all(notice.as_bytes()).await;
                continue;
            }

            let is_first: bool = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM users"
            )
                .fetch_one(&state.pool)
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
                .execute(&state.pool)
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
                    let _ = writer.write_all(notice.as_bytes()).await;
                    authenticated = true;
                }
                Err(_) => {
                    let notice = build_notice("Username already taken.");
                    let _ = writer.write_all(notice.as_bytes()).await;
                }
            }
        } else if let Some(password) = input.strip_prefix("/login ") {
            if password.is_empty() {
                let notice = build_notice("Password cannot be empty.");
                let _ = writer.write_all(notice.as_bytes()).await;
                continue;
            }

            let attempts_key = format!("login_attempts:{}", username);
            let attempts: i32 = redis_conn.get(&attempts_key).unwrap_or(0);
            if attempts >= 3 {
                let ttl: i64 = redis_conn.ttl(&attempts_key).unwrap_or(60);
                let notice = build_notice(&format!("Too many failed attempts. Try again in {} seconds.", ttl));
                let _ = writer.write_all(notice.as_bytes()).await;
                continue;
            }

            match sqlx::query_scalar::<_, String>(
                "SELECT password_hash FROM users WHERE username = $1"
            )
                .bind(&username)
                .fetch_optional(&state.pool)
                .await
            {
                Ok(Some(stored_hash)) => {
                    let parsed = PasswordHash::new(&stored_hash).unwrap();
                    if Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok() {
                        let _: Result<(), _> = redis_conn.del(&attempts_key);
                        let token: String = rand::thread_rng()
                            .sample_iter(&rand::distributions::Alphanumeric)
                            .take(32)
                            .map(char::from)
                            .collect();
                        let _: () = redis_conn
                            .set_ex(format!("session:{}", token), &username, 86400)
                            .unwrap();
                            let notice = build_notice(&format!("Logged in. Token: {}", token));
                            let _ = writer.write_all(notice.as_bytes()).await;
                        authenticated = true;
                    } else {
                        let count: i32 = redis_conn.incr(&attempts_key, 1).unwrap();
                        if count == 1 {
                            let _: () = redis_conn.expire(&attempts_key, 60).unwrap();
                        }
                        let notice = build_notice(&format!("Wrong password. ({}/3 attempts)", count));
                        let _ = writer.write_all(notice.as_bytes()).await;
                    }
                }
                Ok(None) => {
                    let notice = build_notice("User not found. Use /register first.");
                    let _ = writer.write_all(notice.as_bytes()).await;
                }
                Err(_) => {
                    let notice = build_notice("Database error.");
                    let _ = writer.write_all(notice.as_bytes()).await;
                    return;
                }
            }
        } else {
            let notice = build_notice("Authenticate first: /register <password> or /login <password>");
            let _ = writer.write_all(notice.as_bytes()).await;
        }
    }

    let _: () = redis_conn
        .set_ex(format!("presence:{}", username), "online", 30)
        .unwrap();

    let user_role: String = sqlx::query_scalar(
        "SELECT role FROM users WHERE username = $1"
    )
        .bind(&username)
        .fetch_one(&state.pool)
        .await
        .unwrap_or_else(|_| "user".to_string());

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();

    state.register_sender(&username, out_tx.clone()).await;

    state.join_room(&username, "general", out_tx.clone()).await;
    let current_room = "general".to_string();

    let room_id: i32 = state.get_or_create_db_room(&current_room, "system").await;

    let rows = sqlx::query_as::<
        _,
        (String, String, chrono::DateTime<chrono::Utc>, String),
    >(
        "SELECT username, content, created_at, message_type FROM messages WHERE room_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(room_id)
    .fetch_all(&state.pool)
    .await
    .unwrap();

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
        out_tx.send(msg_json).unwrap();
    }

    line.clear();

    let join_msg = ChatMessage {
        username: username.clone(),
        content: format!("Joined the chat"),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: SystemNotification,
    };
    let join_json = serde_json::to_string(&join_msg).unwrap();
    state.send_to_room(&current_room, &join_json).await;

    loop {
        tokio::select! {
            result = reader.read_line(&mut line) => {
                if result.unwrap_or(0) == 0 {
                    break;
                }

                let input = line.trim();

                if input.starts_with('/') {
                    if let Some(target) = input.strip_prefix("/join ") {
                        let room_name = target.trim();
                        if room_name.is_empty() || room_name.len() > 32 || !room_name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                            let notice = build_notice("Invalid room name. Use 1-32 chars (letters, numbers, underscores, hyphens).");
                            let _ = writer.write_all(notice.as_bytes()).await;
                            line.clear();
                            continue;
                        }

                        let new_room_id: i32 = state.get_or_create_db_room(room_name, &username).await;
                        state.join_room(&username, room_name, out_tx.clone()).await;

                        let notice = build_notice(&format!("Joined room '{}'.", room_name));
                        let _ = writer.write_all(notice.as_bytes()).await;

                        let rows = sqlx::query_as::<
                            _,
                            (String, String, chrono::DateTime<chrono::Utc>, String),
                        >(
                            "SELECT username, content, created_at, message_type FROM messages WHERE room_id = $1 ORDER BY created_at DESC LIMIT 50",
                        )
                        .bind(new_room_id)
                        .fetch_all(&state.pool)
                        .await
                        .unwrap();

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
                            let _ = out_tx.send(msg_json);
                        }

                        let leave_notice = ChatMessage {
                            username: username.clone(),
                            content: format!("Joined room '{}'", room_name),
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            message_type: SystemNotification,
                        };
                        let leave_json = serde_json::to_string(&leave_notice).unwrap();
                        state.send_to_room("general", &leave_json).await;

                        let join_notice = ChatMessage {
                            username: username.clone(),
                            content: format!("Joined the room"),
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            message_type: SystemNotification,
                        };
                        let join_json = serde_json::to_string(&join_notice).unwrap();
                        state.send_to_room(room_name, &join_json).await;

                        line.clear();
                        continue;
                    } else if input.starts_with("/rooms") {
                        let rooms = state.list_db_rooms().await;
                        if rooms.is_empty() {
                            let notice = build_notice("No rooms available.");
                            let _ = writer.write_all(notice.as_bytes()).await;
                        } else {
                            let room_list = rooms.join(", ");
                            let notice = build_notice(&format!("Available rooms: {}", room_list));
                            let _ = writer.write_all(notice.as_bytes()).await;
                        }
                        line.clear();
                        continue;
                    } else if let Some(rest) = input.strip_prefix("/msg ") {
                        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                        if parts.len() < 2 {
                            let notice = build_notice("Usage: /msg <username> <message>");
                            let _ = writer.write_all(notice.as_bytes()).await;
                            line.clear();
                            continue;
                        }
                        let target = parts[0].trim();
                        let dm_text = parts[1].trim();

                        if target == username {
                            let notice = build_notice("You cannot DM yourself.");
                            let _ = writer.write_all(notice.as_bytes()).await;
                            line.clear();
                            continue;
                        }

                        if dm_text.is_empty() {
                            let notice = build_notice("Message cannot be empty.");
                            let _ = writer.write_all(notice.as_bytes()).await;
                            line.clear();
                            continue;
                        }

                        let target_exists: bool = sqlx::query_scalar::<_, i32>(
                            "SELECT 1 FROM users WHERE username = $1",
                        )
                        .bind(target)
                        .fetch_optional(&state.pool)
                        .await
                        .unwrap()
                        .is_some();

                        if !target_exists {
                            let notice = build_notice(&format!("User '{}' not found.", target));
                            let _ = writer.write_all(notice.as_bytes()).await;
                            line.clear();
                            continue;
                        }

                        let mut users = vec![username.clone(), target.to_string()];
                        users.sort();
                        let dm_room = format!("__dm__{}", users.join("_"));

                        let room_id: i32 = state.get_or_create_db_room(&dm_room, &username).await;

                        state.join_room(&username, &dm_room, out_tx.clone()).await;

                        let notice = build_notice(&format!("Now in DM with {}.", target));
                        let _ = writer.write_all(notice.as_bytes()).await;

                        let rows = sqlx::query_as::<
                            _,
                            (String, String, chrono::DateTime<chrono::Utc>, String),
                        >(
                            "SELECT username, content, created_at, message_type FROM messages WHERE room_id = $1 ORDER BY created_at DESC LIMIT 50",
                        )
                        .bind(room_id)
                        .fetch_all(&state.pool)
                        .await
                        .unwrap();

                        for row in rows.into_iter().rev() {
                            let msg = ChatMessage {
                                username: row.0,
                                content: row.1,
                                timestamp: row.2.format("%H:%M:%S").to_string(),
                                message_type: match row.3.as_str() {
                                    "UserMessage" => MessageType::UserMessage,
                                    _ => MessageType::SystemNotification,
                                },
                            };
                            let msg_json = serde_json::to_string(&msg).unwrap();
                            let _ = out_tx.send(msg_json);
                        }

                        let dm_msg = ChatMessage {
                            username: username.clone(),
                            content: dm_text.to_string(),
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            message_type: MessageType::UserMessage,
                        };
                        let dm_json = serde_json::to_string(&dm_msg).unwrap();

                        sqlx::query("INSERT INTO messages (username, content, message_type, room_id) VALUES ($1, $2, $3, $4)")
                            .bind(&dm_msg.username)
                            .bind(&dm_msg.content)
                            .bind((dm_msg.message_type).to_string())
                            .bind(room_id)
                            .execute(&state.pool)
                            .await
                            .unwrap();

                        state.send_to_room(&dm_room, &dm_json).await;

                        if let Some(target_tx) = state.get_sender(target).await {
                            let whisper = build_notice(&format!("DM from {}: '{}'. /join {} to reply.", username, dm_text, dm_room));
                            let _ = target_tx.send(whisper);
                        }

                        line.clear();
                        continue;
                    } else if let Some(target) = input.strip_prefix("/mute ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /mute.");
                            let _ = writer.write_all(notice.as_bytes()).await;
                        } else {
                            let parts: Vec<&str> = target.splitn(2, ' ').collect();
                            if parts.len() < 2 {
                                let notice = build_notice("Usage: /mute <username> <minutes>");
                                let _ = writer.write_all(notice.as_bytes()).await;
                            } else {
                                let target_user = parts[0].trim();
                                let minutes: u64 = parts[1].trim().parse().unwrap_or(5);
                                let seconds = minutes * 60;
                                let _: Result<(), _> = redis_conn.set_ex(format!("muted:{}", target_user), "1", seconds);
                                let notice = build_notice(&format!("Muted {} for {} minutes.", target_user, minutes));
                                let _ = writer.write_all(notice.as_bytes()).await;
                                sqlx::query("INSERT INTO audit_log (actor, action, target, details) VALUES ($1, 'mute', $2, $3)")
                                    .bind(&username)
                                    .bind(target_user)
                                    .bind(format!("{} minutes", minutes))
                                    .execute(&state.pool)
                                    .await
                                    .unwrap();
                            }
                        }
                    } else if let Some(target) = input.strip_prefix("/unmute ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /unmute.");
                            let _ = writer.write_all(notice.as_bytes()).await;
                        } else {
                            let target_user = target.trim();
                            let _: Result<usize, _> = redis_conn.del(format!("muted:{}", target_user));
                            let notice = build_notice(&format!("Unmuted {}.", target_user));
                            let _ = writer.write_all(notice.as_bytes()).await;
                            sqlx::query("INSERT INTO audit_log (actor, action, target) VALUES ($1, 'unmute', $2)")
                                .bind(&username)
                                .bind(target_user)
                                .execute(&state.pool)
                                .await
                                .unwrap();
                        }
                    } else if let Some(target) = input.strip_prefix("/ban ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /ban.");
                            let _ = writer.write_all(notice.as_bytes()).await;
                        } else {
                            let parts: Vec<&str> = target.splitn(2, ' ').collect();
                            let target_user = parts[0].trim();
                            let reason = if parts.len() > 1 { parts[1].trim() } else { "No reason" };
                            match sqlx::query("INSERT INTO bans (username, banned_by, reason) VALUES ($1, $2, $3)")
                                .bind(target_user)
                                .bind(&username)
                                .bind(reason)
                                .execute(&state.pool)
                                .await
                            {
                                Ok(_) => {
                                    let notice = build_notice(&format!("Banned {} (reason: {}).", target_user, reason));
                                    let _ = writer.write_all(notice.as_bytes()).await;
                                    sqlx::query("INSERT INTO audit_log (actor, action, target, details) VALUES ($1, 'ban', $2, $3)")
                                        .bind(&username)
                                        .bind(target_user)
                                        .bind(reason)
                                        .execute(&state.pool)
                                        .await
                                        .unwrap();
                                }
                                Err(_) => {
                                    let notice = build_notice(&format!("User '{}' is not registered.", target_user));
                                    let _ = writer.write_all(notice.as_bytes()).await;
                                }
                            }
                        }
                    } else if let Some(target) = input.strip_prefix("/unban ") {
                        if user_role != "admin" {
                            let notice = build_notice("Only admins can use /unban.");
                            let _ = writer.write_all(notice.as_bytes()).await;
                        } else {
                            let target_user = target.trim();
                            sqlx::query("DELETE FROM bans WHERE username = $1")
                                .bind(target_user)
                                .execute(&state.pool)
                                .await
                                .unwrap();
                            let notice = build_notice(&format!("Unbanned {}.", target_user));
                            let _ = writer.write_all(notice.as_bytes()).await;
                            sqlx::query("INSERT INTO audit_log (actor, action, target) VALUES ($1, 'unban', $2)")
                                .bind(&username)
                                .bind(target_user)
                                .execute(&state.pool)
                                .await
                                .unwrap();
                        }
                    } else if input == "/help" {
                        let notice = build_notice(
                            "Commands: /join <room>, /rooms, /msg <user> <text>, /help | Admin: /mute, /unmute, /ban, /unban"
                        );
                        let _ = writer.write_all(notice.as_bytes()).await;
                        line.clear();
                        continue;
                    } else {
                        let notice = build_notice("Unknown command. Try /help.");
                        let _ = writer.write_all(notice.as_bytes()).await;
                    }
                    line.clear();
                    continue;
                }

                if input.len() > 4096 {
                    let notice = build_notice("Message too long (max 4096 characters).");
                    let _ = writer.write_all(notice.as_bytes()).await;
                    line.clear();
                    continue;
                }

                let muted: Option<String> = redis_conn.get(format!("muted:{}", username)).ok();
                if muted.is_some() {
                    let notice = build_notice("You are muted and cannot send messages.");
                    let _ = writer.write_all(notice.as_bytes()).await;
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
                    let _: Result<bool, _> = redis_conn
                        .expire(format!("ratelimit:{}", msg.username), 10);
                }
                if count > 20 {
                    let warn = ChatMessage {
                        username: "Server".to_string(),
                        content: "Rate limit exceeded. Slow down.".to_string(),
                        timestamp: Local::now().format("%H:%M:%S").to_string(),
                        message_type: SystemNotification,
                    };
                    let warn_json = serde_json::to_string(&warn).unwrap();
                    let _ = writer.write_all(warn_json.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                    line.clear();
                    continue;
                }

                let user_room = state.get_user_room(&username).await;
                let room_id: i32 = state.get_or_create_db_room(&user_room, "system").await;

                state.send_to_room(&user_room, &json).await;

                sqlx::query("INSERT INTO messages (username, content, message_type, room_id) VALUES ($1, $2, $3, $4)")
                    .bind(&msg.username)
                    .bind(&msg.content)
                    .bind((msg.message_type).to_string())
                    .bind(room_id)
                    .execute(&state.pool)
                    .await
                    .unwrap();

                line.clear();
            }

            msg = out_rx.recv() => {
                if let Some(msg) = msg {
                    let _ = writer.write_all(msg.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                }
            }
        }
    }

    let _: Result<usize, _> = redis_conn.del(format!("presence:{}", username));

    let user_room = state.get_user_room(&username).await;
    let leave_msg = ChatMessage {
        username: username.clone(),
        content: "Left the chat".to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: SystemNotification,
    };
    let leave_json = serde_json::to_string(&leave_msg).unwrap();
    state.send_to_room(&user_room, &leave_json).await;

    state.leave_all_rooms(&username).await;
    state.unregister_sender(&username).await;

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

    let state = AppState::new(pool, redis_client);

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

        let state = state.clone();
        let redis_client = state.redis_client.clone();

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
                        handle_connection(tls_stream, addr, state).await
                    });
                }
                Err(e) => {
                    println!("  └─ TLS handshake failed: {e}");
                }
            }
        } else {
            tokio::spawn(async move {
                handle_connection(socket, addr, state).await
            });
        }
    }
}
