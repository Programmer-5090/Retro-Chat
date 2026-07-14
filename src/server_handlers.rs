use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{
    io::{ AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader },
    sync::mpsc,
    time::interval,
};
use chrono::Local;
use redis::Commands;
use argon2::{
    password_hash::{ SaltString, PasswordHash, PasswordHasher, PasswordVerifier },
    Argon2,
};
use rand::Rng;

use crate::{
    ChatMessage,
    MessageType,
    build_notice,
    build_read_receipt,
    dm_display_name,
    generate_message_id,
    AppState,
};

async fn send_notice(writer: &mut (impl AsyncWrite + Unpin), text: &str) {
    let notice = build_notice(text);
    let _ = writer.write_all(notice.as_bytes()).await;
}

fn validate_username(username: &str) -> bool {
    let len = username.len();
    len >= 3 && len <= 32 && username.chars().all(|c| c.is_alphanumeric() || c == '_')
}

async fn check_banned(state: &AppState, username: &str) -> bool {
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM bans WHERE username = $1")
        .bind(username)
        .fetch_optional(&state.pool).await
        .unwrap()
        .is_some()
}

async fn authenticate_user(
    reader: &mut BufReader<impl AsyncRead + Unpin>,
    writer: &mut (impl AsyncWrite + Unpin),
    state: &AppState,
    redis_conn: &mut redis::Connection,
    username: &str
) -> bool {
    let mut authenticated = false;
    let mut line = String::new();

    while !authenticated {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return false;
        }
        let input = line.trim();

        if let Some(password) = input.strip_prefix("/register ") {
            if password.len() < 8 {
                send_notice(writer, "Password must be at least 8 characters.").await;
                continue;
            }
            let is_first: bool =
                sqlx
                    ::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
                    .fetch_one(&state.pool).await
                    .unwrap_or(0) == 0;
            let role = if is_first { "admin" } else { "user" };
            let salt = SaltString::generate(&mut rand::rngs::OsRng);
            let hash = Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .unwrap()
                .to_string();
            match
                sqlx
                    ::query("INSERT INTO users (username, password_hash, role) VALUES ($1, $2, $3)")
                    .bind(username)
                    .bind(&hash)
                    .bind(role)
                    .execute(&state.pool).await
            {
                Ok(_) => {
                    let token: String = rand
                        ::thread_rng()
                        .sample_iter(&rand::distributions::Alphanumeric)
                        .take(32)
                        .map(char::from)
                        .collect();
                    let _: () = redis_conn
                        .set_ex(format!("session:{}", token), username, 86400)
                        .unwrap();
                    send_notice(
                        writer,
                        &format!("Registered and logged in. Token: {}", token)
                    ).await;
                    authenticated = true;
                }
                Err(_) => send_notice(writer, "Username already taken.").await,
            }
        } else if let Some(password) = input.strip_prefix("/login ") {
            if password.is_empty() {
                send_notice(writer, "Password cannot be empty.").await;
                continue;
            }
            let attempts_key = format!("login_attempts:{}", username);
            let attempts: i32 = redis_conn.get(&attempts_key).unwrap_or(0);
            if attempts >= 3 {
                let ttl: i64 = redis_conn.ttl(&attempts_key).unwrap_or(60);
                send_notice(
                    writer,
                    &format!("Too many failed attempts. Try again in {} seconds.", ttl)
                ).await;
                continue;
            }
            match
                sqlx
                    ::query_scalar::<_, String>(
                        "SELECT password_hash FROM users WHERE username = $1"
                    )
                    .bind(username)
                    .fetch_optional(&state.pool).await
            {
                Ok(Some(stored_hash)) => {
                    let parsed = PasswordHash::new(&stored_hash).unwrap();
                    if Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok() {
                        let _: Result<(), _> = redis_conn.del(&attempts_key);
                        let token: String = rand
                            ::thread_rng()
                            .sample_iter(&rand::distributions::Alphanumeric)
                            .take(32)
                            .map(char::from)
                            .collect();
                        let _: () = redis_conn
                            .set_ex(format!("session:{}", token), username, 86400)
                            .unwrap();
                        send_notice(writer, &format!("Logged in. Token: {}", token)).await;
                        authenticated = true;
                    } else {
                        let count: i32 = redis_conn.incr(&attempts_key, 1).unwrap();
                        if count == 1 {
                            let _: () = redis_conn.expire(&attempts_key, 60).unwrap();
                        }
                        send_notice(
                            writer,
                            &format!("Wrong password. ({}/3 attempts)", count)
                        ).await;
                    }
                }
                Ok(None) => send_notice(writer, "User not found. Use /register first.").await,
                Err(_) => {
                    send_notice(writer, "Database error.").await;
                    return false;
                }
            }
        } else {
            send_notice(
                writer,
                "Authenticate first: /register <password> or /login <password>"
            ).await;
        }
    }
    true
}

async fn send_room_list(state: &AppState, username: &str, out_tx: &mpsc::UnboundedSender<String>) {
    let memberships = state.get_user_room_memberships(username).await;
    let room_names = if memberships.is_empty() {
        "general".to_string()
    } else {
        memberships.join(",")
    };
    let msg = ChatMessage {
        id: String::new(),
        username: "Server".to_string(),
        content: room_names,
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::RoomList,
        room: String::new(),
        is_history: false,
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let _ = out_tx.send(json);
}

async fn handle_leave_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>
) {
    let current_room = state.get_user_room(username).await;
    if current_room == "general" {
        send_notice(writer, "Cannot leave the default room.").await;
        return;
    }

    state.remove_room_membership(username, &current_room).await;
    state.unsubscribe_room(username, &current_room).await;

    let leave_notice = ChatMessage {
        id: String::new(),
        username: username.to_string(),
        content: "Left the room".to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
        room: current_room.clone(),
        is_history: false,
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    };
    let leave_json = serde_json::to_string(&leave_notice).unwrap();
    state.send_to_room(&current_room, &leave_json).await;

    let fallback = state.get_last_room(username).await.unwrap_or_else(|| "general".to_string());
    state.subscribe_room(username, &fallback, out_tx.clone()).await;
    state.set_active_room(username, &fallback).await;
    send_notice(writer, &format!("Left '{}'. Now in '{}'.", dm_display_name(&current_room, username), dm_display_name(&fallback, username))).await;
    replay_history(state, &fallback, out_tx).await;
    send_room_list(state, username, out_tx).await;
}

async fn handle_switch_command(state: &AppState, username: &str, input: &str) {
    let room_name = input.trim().to_string();
    if room_name.is_empty() {
        return;
    }
    state.set_active_room(username, &room_name).await;
}

async fn replay_history(state: &AppState, room_name: &str, out_tx: &mpsc::UnboundedSender<String>) {
    let room_id: i32 = state.get_or_create_db_room(room_name, "system").await;
    let rows = sqlx
        ::query_as::<_, (String, String, chrono::DateTime<chrono::Utc>, String, Option<String>, Option<String>, Option<i32>, Option<i32>)>(
            "SELECT username, content, created_at, message_type, image_url, thumb_url, width, height FROM messages WHERE room_id = $1 ORDER BY created_at DESC LIMIT 50"
        )
        .bind(room_id)
        .fetch_all(&state.pool).await
        .unwrap();

    for row in rows.into_iter().rev() {
        let msg = ChatMessage {
            id: generate_message_id(),
            username: row.0,
            content: row.1,
            timestamp: row.2.format("%H:%M:%S").to_string(),
            message_type: match row.3.as_str() {
                "UserMessage" => MessageType::UserMessage,
                "ImageMessage" => MessageType::ImageMessage,
                _ => MessageType::SystemNotification,
            },
            room: room_name.to_string(),
            is_history: true,
            image_url: row.4.unwrap_or_default(),
            thumb_url: row.5.unwrap_or_default(),
            width: row.6.unwrap_or(0) as u32,
            height: row.7.unwrap_or(0) as u32,
        };
        let msg_json = serde_json::to_string(&msg).unwrap();
        let _ = out_tx.send(msg_json);
    }
}

async fn handle_join_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>,
    input: &str
) {
    let room_name = input.trim();
    if
        room_name.is_empty() ||
        room_name.len() > 32 ||
        !room_name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        send_notice(
            writer,
            "Invalid room name. Use 1-32 chars (letters, numbers, underscores, hyphens)."
        ).await;
        return;
    }
    let old_room = state.get_user_room(username).await;
    let _new_room_id: i32 = state.get_or_create_db_room(room_name, username).await;
    state.subscribe_room(username, room_name, out_tx.clone()).await;
    state.set_active_room(username, room_name).await;
    state.save_room_membership(username, room_name).await;
    send_notice(writer, &format!("Joined room '{}'.", dm_display_name(room_name, username))).await;
    replay_history(state, room_name, out_tx).await;
    send_room_list(state, username, out_tx).await;

    // The user stays live-subscribed to `old_room` (they can still
    // get messages/unread badges there) — they've only changed which room
    // is active, so there's no "left the room" notice to send anymore.
    let dm_move = old_room.starts_with("__dm__") || room_name.starts_with("__dm__");
    if !dm_move {
        let join_notice = ChatMessage {
            id: String::new(),
            username: username.to_string(),
            content: "Joined the room".to_string(),
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            message_type: MessageType::SystemNotification,
            room: room_name.to_string(),
            is_history: false,
            image_url: String::new(),
            thumb_url: String::new(),
            width: 0,
            height: 0,
        };
        let join_json = serde_json::to_string(&join_notice).unwrap();
        state.send_to_room(room_name, &join_json).await;
    }
}

async fn handle_msg_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>,
    input: &str
) {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    if parts.len() < 2 {
        send_notice(writer, "Usage: /msg <username> <message>").await;
        return;
    }
    let target = parts[0].trim();
    let dm_text = parts[1].trim();

    if target == username {
        send_notice(writer, "You cannot DM yourself.").await;
        return;
    }
    if dm_text.is_empty() {
        send_notice(writer, "Message cannot be empty.").await;
        return;
    }

    let target_exists: bool = sqlx
        ::query_scalar::<_, i32>("SELECT 1 FROM users WHERE username = $1")
        .bind(target)
        .fetch_optional(&state.pool).await
        .unwrap()
        .is_some();

    if !target_exists {
        send_notice(writer, &format!("User '{}' not found.", target)).await;
        return;
    }

    let mut users = vec![username.to_string(), target.to_string()];
    users.sort();
    let dm_room = format!("__dm__{}", users.join("_"));

    let room_id: i32 = state.get_or_create_db_room(&dm_room, username).await;
    state.subscribe_room(username, &dm_room, out_tx.clone()).await;
    state.set_active_room(username, &dm_room).await;
    state.save_room_membership(username, &dm_room).await;
    send_notice(writer, &format!("Now in DM with {}.", target)).await;
    replay_history(state, &dm_room, out_tx).await;
    send_room_list(state, username, out_tx).await;

    let dm_msg = ChatMessage {
        id: generate_message_id(),
        username: username.to_string(),
        content: dm_text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::UserMessage,
        room: dm_room.clone(),
        is_history: false,
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    };
    let dm_json = serde_json::to_string(&dm_msg).unwrap();

    sqlx::query(
        "INSERT INTO messages (username, content, message_type, room_id) VALUES ($1, $2, $3, $4)"
    )
        .bind(&dm_msg.username)
        .bind(&dm_msg.content)
        .bind(dm_msg.message_type.to_string())
        .bind(room_id)
        .execute(&state.pool).await
        .unwrap();

    state.save_room_membership(target, &dm_room).await;

    if let Some(target_tx) = state.get_sender(target).await {
        state.subscribe_room(target, &dm_room, target_tx.clone()).await;
        let whisper = build_notice(
            &format!("DM from {}: '{}'. Check your sidebar to join.", username, dm_text)
        );
        let _ = target_tx.send(whisper);
        send_room_list(state, target, &target_tx).await;
    }

    state.send_to_room(&dm_room, &dm_json).await;
}

async fn handle_mute_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    redis_conn: &mut redis::Connection,
    username: &str,
    user_role: &str,
    input: &str
) {
    if user_role != "admin" {
        send_notice(writer, "Only admins can use /mute.").await;
        return;
    }
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    if parts.len() < 2 {
        send_notice(writer, "Usage: /mute <username> <minutes>").await;
        return;
    }
    let target_user = parts[0].trim();
    let minutes: u64 = parts[1].trim().parse().unwrap_or(5);
    let _: Result<(), _> = redis_conn.set_ex(format!("muted:{}", target_user), "1", minutes * 60);
    send_notice(writer, &format!("Muted {} for {} minutes.", target_user, minutes)).await;
    sqlx::query(
        "INSERT INTO audit_log (actor, action, target, details) VALUES ($1, 'mute', $2, $3)"
    )
        .bind(username)
        .bind(target_user)
        .bind(format!("{} minutes", minutes))
        .execute(&state.pool).await
        .unwrap();
}

async fn handle_unmute_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    redis_conn: &mut redis::Connection,
    username: &str,
    user_role: &str,
    input: &str
) {
    if user_role != "admin" {
        send_notice(writer, "Only admins can use /unmute.").await;
        return;
    }
    let target_user = input.trim();
    let _: Result<usize, _> = redis_conn.del(format!("muted:{}", target_user));
    send_notice(writer, &format!("Unmuted {}.", target_user)).await;
    sqlx::query("INSERT INTO audit_log (actor, action, target) VALUES ($1, 'unmute', $2)")
        .bind(username)
        .bind(target_user)
        .execute(&state.pool).await
        .unwrap();
}

async fn handle_ban_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    username: &str,
    user_role: &str,
    input: &str
) {
    if user_role != "admin" {
        send_notice(writer, "Only admins can use /ban.").await;
        return;
    }
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let target_user = parts[0].trim();
    let reason = if parts.len() > 1 { parts[1].trim() } else { "No reason" };
    match
        sqlx
            ::query("INSERT INTO bans (username, banned_by, reason) VALUES ($1, $2, $3)")
            .bind(target_user)
            .bind(username)
            .bind(reason)
            .execute(&state.pool).await
    {
        Ok(_) => {
            send_notice(writer, &format!("Banned {} (reason: {}).", target_user, reason)).await;
            sqlx::query(
                "INSERT INTO audit_log (actor, action, target, details) VALUES ($1, 'ban', $2, $3)"
            )
                .bind(username)
                .bind(target_user)
                .bind(reason)
                .execute(&state.pool).await
                .unwrap();
        }
        Err(_) => send_notice(writer, &format!("User '{}' is not registered.", target_user)).await,
    }
}

async fn handle_unban_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    username: &str,
    user_role: &str,
    input: &str
) {
    if user_role != "admin" {
        send_notice(writer, "Only admins can use /unban.").await;
        return;
    }
    let target_user = input.trim();
    sqlx::query("DELETE FROM bans WHERE username = $1")
        .bind(target_user)
        .execute(&state.pool).await
        .unwrap();
    send_notice(writer, &format!("Unbanned {}.", target_user)).await;
    sqlx::query("INSERT INTO audit_log (actor, action, target) VALUES ($1, 'unban', $2)")
        .bind(username)
        .bind(target_user)
        .execute(&state.pool).await
        .unwrap();
}

async fn handle_regular_message(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    redis_conn: &mut redis::Connection,
    username: &str,
    _out_tx: &mpsc::UnboundedSender<String>,
    input: &str
) {
    if input.len() > 4096 {
        send_notice(writer, "Message too long (max 4096 characters).").await;
        return;
    }

    let muted: Option<String> = redis_conn.get(format!("muted:{}", username)).ok();
    if muted.is_some() {
        send_notice(writer, "You are muted and cannot send messages.").await;
        return;
    }

    let user_room = state.get_user_room(username).await;

    let msg = ChatMessage {
        id: generate_message_id(),
        username: username.to_string(),
        content: input.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::UserMessage,
        room: user_room.clone(),
        is_history: false,
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    };
    let json = serde_json::to_string(&msg).unwrap();

    let count: i32 = redis_conn.incr(format!("ratelimit:{}", msg.username), 1).unwrap();
    if count == 1 {
        let _: Result<bool, _> = redis_conn.expire(format!("ratelimit:{}", msg.username), 10);
    }
    if count > 20 {
        let warn = ChatMessage {
            id: String::new(),
            username: "Server".to_string(),
            content: "Rate limit exceeded. Slow down.".to_string(),
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            message_type: MessageType::SystemNotification,
            room: String::new(),
            is_history: false,
            image_url: String::new(),
            thumb_url: String::new(),
            width: 0,
            height: 0,
        };
        let warn_json = serde_json::to_string(&warn).unwrap();
        let _ = writer.write_all(warn_json.as_bytes()).await;
        let _ = writer.write_all(b"\n").await;
        return;
    }

    let room_id: i32 = state.get_or_create_db_room(&user_room, "system").await;
    state.send_to_room(&user_room, &json).await;

    sqlx::query(
        "INSERT INTO messages (username, content, message_type, room_id) VALUES ($1, $2, $3, $4)"
    )
        .bind(&msg.username)
        .bind(&msg.content)
        .bind(msg.message_type.to_string())
        .bind(room_id)
        .execute(&state.pool).await
        .unwrap();
}

/// Handles the client's `/read <room> <id1,id2,...>` command by relaying a
/// read receipt to everyone currently in that room. Silent on malformed
/// input
async fn handle_read_command(state: &AppState, username: &str, input: &str) {
    let mut parts = input.splitn(2, ' ');
    let room = match parts.next() {
        Some(r) if !r.is_empty() => r,
        _ => {
            return;
        }
    };
    let ids_csv = parts.next().unwrap_or("").trim();
    if ids_csv.is_empty() {
        return;
    }
    let ids: Vec<String> = ids_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return;
    }

    let receipt = build_read_receipt(username, room, &ids);
    state.send_to_room(room, &receipt).await;
}

async fn cleanup_disconnect(state: &AppState, redis_conn: &mut redis::Connection, username: &str) {
    let _: Result<usize, _> = redis_conn.del(format!("presence:{}", username));
    let user_room = state.get_user_room(username).await;
    let _ = redis_conn.del::<_, usize>(format!("typing:{}:{}", user_room, username));

    let subscribed_rooms = state.get_user_subscribed_rooms(username).await;
    for room in &subscribed_rooms {
        let leave_msg = ChatMessage {
            id: String::new(),
            username: username.to_string(),
            content: "Left the chat".to_string(),
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            message_type: MessageType::SystemNotification,
            room: room.clone(),
            is_history: false,
            image_url: String::new(),
            thumb_url: String::new(),
            width: 0,
            height: 0,
        };
        let leave_json = serde_json::to_string(&leave_msg).unwrap();
        state.send_to_room(room, &leave_json).await;
    }

    state.leave_all_rooms(username).await;
    state.unregister_sender(username).await;
}

pub async fn handle_connection<S>(stream: S, _addr: SocketAddr, state: Arc<AppState>)
    where S: AsyncRead + AsyncWrite + Unpin + Send + 'static
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut redis_conn = state.redis_client.get_connection().unwrap();

    let mut username = String::new();
    if reader.read_line(&mut username).await.unwrap_or(0) == 0 {
        return;
    }
    let username = username.trim().to_string();

    if !validate_username(&username) {
        send_notice(
            &mut writer,
            "Username must be 3-32 characters (letters, numbers, underscores)."
        ).await;
        return;
    }

    if check_banned(&state, &username).await {
        send_notice(&mut writer, "Your account has been banned.").await;
        return;
    }

    match
        sqlx
            ::query_scalar::<_, String>("SELECT password_hash FROM users WHERE username = $1")
            .bind(&username)
            .fetch_optional(&state.pool).await
    {
        Ok(Some(_)) => send_notice(&mut writer, "Password required. Send: /login <password>").await,
        Ok(None) =>
            send_notice(&mut writer, "Register a password. Send: /register <password>").await,
        Err(_) => {
            send_notice(&mut writer, "Database error.").await;
            return;
        }
    }

    if !authenticate_user(&mut reader, &mut writer, &state, &mut redis_conn, &username).await {
        return;
    }

    let _: () = redis_conn.set_ex(format!("presence:{}", username), "online", 30).unwrap();

    let user_role: String = sqlx
        ::query_scalar("SELECT role FROM users WHERE username = $1")
        .bind(&username)
        .fetch_one(&state.pool).await
        .unwrap_or_else(|_| "user".to_string());

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    state.register_sender(&username, out_tx.clone()).await;

    let memberships = state.get_user_room_memberships(&username).await;
    let current_room = state
        .get_last_room(&username).await
        .unwrap_or_else(|| "general".to_string());

    // Subscribe to every room this user belongs to, not just the one
    // they'll land on — otherwise messages sent to their other rooms
    // (DMs, other channels) never reach their socket at all.
    for room in &memberships {
        state.subscribe_room(&username, room, out_tx.clone()).await;
    }
    if !memberships.iter().any(|r| r == &current_room) {
        state.subscribe_room(&username, &current_room, out_tx.clone()).await;
    }
    state.set_active_room(&username, &current_room).await;
    state.save_room_membership(&username, &current_room).await;

    replay_history(&state, &current_room, &out_tx).await;
    send_room_list(&state, &username, &out_tx).await;

    let active_msg = ChatMessage {
        id: String::new(),
        username: String::new(),
        content: current_room.clone(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SetActiveRoom,
        room: String::new(),
        is_history: false,
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    };
    let active_json = serde_json::to_string(&active_msg).unwrap();
    let _ = out_tx.send(active_json);

    let join_msg = ChatMessage {
        id: String::new(),
        username: username.clone(),
        content: "Joined the chat".to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
        room: current_room.clone(),
        is_history: false,
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    };
    let join_json = serde_json::to_string(&join_msg).unwrap();
    state.send_to_room(&current_room, &join_json).await;

    // Tell the newly-connected client who's online globally (from Redis).
    let online: Vec<String> = state
        .get_online_users().await
        .into_iter()
        .filter(|u| u != &username)
        .collect();
    let sync_msg = ChatMessage {
        id: String::new(),
        username: String::new(),
        content: online.join(","),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::PresenceSync,
        room: current_room.clone(),
        is_history: false,
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    };
    let sync_json = serde_json::to_string(&sync_msg).unwrap();
    let _ = out_tx.send(sync_json);

    // Sync current typing state from Redis
    let typing_keys: Vec<String> = redis
        ::cmd("KEYS")
        .arg(format!("typing:{}:*", current_room))
        .query(&mut redis_conn)
        .unwrap_or_default();
    for key in &typing_keys {
        if let Some(u) = key.strip_prefix(&format!("typing:{}:", current_room)) {
            if u != username {
                let t_msg = ChatMessage {
                    id: String::new(),
                    username: u.to_string(),
                    content: String::new(),
                    timestamp: Local::now().format("%H:%M:%S").to_string(),
                    message_type: MessageType::TypingNotification,
                    room: current_room.clone(),
                    is_history: false,
                    image_url: String::new(),
                    thumb_url: String::new(),
                    width: 0,
                    height: 0,
                };
                let t_json = serde_json::to_string(&t_msg).unwrap();
                let _ = out_tx.send(t_json);
            }
        }
    }

    let mut presence_heartbeat = interval(Duration::from_secs(10));
    presence_heartbeat.tick().await; // first tick fires immediately; consume it, key is already fresh from connect

    let mut line = String::new();
    loop {
        tokio::select! {
            _ = presence_heartbeat.tick() => {
                let _: Result<(), _> = redis_conn.set_ex(
                    format!("presence:{}", username),
                    "online",
                    30,
                );
            }

            result = reader.read_line(&mut line) => {
                if result.unwrap_or(0) == 0 {
                    break;
                }

                let input = line.trim();

                if input.starts_with('/') {
                    if let Some(args) = input.strip_prefix("/join ") {
                        handle_join_command(&state, &mut writer, &username, &out_tx, args).await;
                    } else if input == "/leave" {
                        handle_leave_command(&state, &mut writer, &username, &out_tx).await;
                    } else if input.starts_with("/rooms") {
                        let rooms = state.list_db_rooms().await;
                        if rooms.is_empty() {
                            send_notice(&mut writer, "No rooms available.").await;
                        } else {
                            send_notice(&mut writer, &format!("Available rooms: {}", rooms.join(", "))).await;
                        }
                    } else if let Some(args) = input.strip_prefix("/msg ") {
                        handle_msg_command(&state, &mut writer, &username, &out_tx, args).await;
                    } else if let Some(args) = input.strip_prefix("/read ") {
                        handle_read_command(&state, &username, args).await;
                    } else if let Some(args) = input.strip_prefix("/mute ") {
                        handle_mute_command(&state, &mut writer, &mut redis_conn, &username, &user_role, args).await;
                    } else if let Some(args) = input.strip_prefix("/unmute ") {
                        handle_unmute_command(&state, &mut writer, &mut redis_conn, &username, &user_role, args).await;
                    } else if let Some(args) = input.strip_prefix("/ban ") {
                        handle_ban_command(&state, &mut writer, &username, &user_role, args).await;
                    } else if let Some(args) = input.strip_prefix("/unban ") {
                        handle_unban_command(&state, &mut writer, &username, &user_role, args).await;
                    } else if input.starts_with("/typing") {
                        let room = if let Some(r) = input.strip_prefix("/typing ")
                            .and_then(|r| {
                                let t = r.trim();
                                if t.is_empty() { None } else { Some(t.to_string()) }
                            }) {
                            r
                        } else {
                            state.get_last_room(&username).await.unwrap_or_else(|| "general".to_string())
                        };
                        let _: () = redis_conn.set_ex(
                            format!("typing:{}:{}", room, username),
                            "1", 10,
                        ).unwrap();
                        let typing_msg = ChatMessage {
                            id: String::new(),
                            username: username.clone(),
                            content: String::new(),
                            timestamp: Local::now().format("%H:%M:%S").to_string(),
                            message_type: MessageType::TypingNotification,
                            room: room.clone(),
                            is_history: false,
                            image_url: String::new(),
                            thumb_url: String::new(),
                            width: 0,
                            height: 0,
                        };
                        let typing_json = serde_json::to_string(&typing_msg).unwrap();
                        state.send_to_room(&room, &typing_json).await;
                    } else if let Some(args) = input.strip_prefix("/image ") {
                        let parts: Vec<&str> = args.splitn(4, ' ').collect();
                        if parts.len() >= 2 {
                            let image_url = parts[0].to_string();
                            let thumb_url = parts[1].to_string();
                            let width: u32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                            let height: u32 = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
                            let image_msg = ChatMessage {
                                id: generate_message_id(),
                                username: username.clone(),
                                content: String::new(),
                                timestamp: Local::now().format("%H:%M:%S").to_string(),
                                message_type: MessageType::ImageMessage,
                                room: current_room.clone(),
                                is_history: false,
                                image_url,
                                thumb_url,
                                width,
                                height,
                            };
                            let image_json = serde_json::to_string(&image_msg).unwrap();
                            let room_id = state.get_or_create_db_room(&current_room, &username).await;
                            sqlx::query(
                                "INSERT INTO messages (room_id, username, content, message_type, image_url, thumb_url, width, height) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
                            )
                            .bind(room_id)
                            .bind(&username)
                            .bind(&image_msg.content)
                            .bind("ImageMessage")
                            .bind(&image_msg.image_url)
                            .bind(&image_msg.thumb_url)
                            .bind(image_msg.width as i32)
                            .bind(image_msg.height as i32)
                            .execute(&state.pool).await.ok();
                            state.send_to_room(&current_room, &image_json).await;
                        } else {
                            send_notice(&mut writer, "Usage: /image <url> <thumb_url> [width] [height]").await;
                        }
                    } else if let Some(args) = input.strip_prefix("/switch ") {
                        handle_switch_command(&state, &username, args).await;
                    } else if input == "/help" {
                        send_notice(&mut writer, "Commands: /join <room>, /rooms, /msg <user> <text>, /image <url> <thumb_url>, /help | Admin: /mute, /unmute, /ban, /unban").await;
                    } else {
                        send_notice(&mut writer, "Unknown command. Try /help.").await;
                    }
                    line.clear();
                    continue;
                }

                handle_regular_message(&state, &mut writer, &mut redis_conn, &username, &out_tx, input).await;
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

    cleanup_disconnect(&state, &mut redis_conn, &username).await;
    println!("└─[{}] {} disconnected", Local::now().format("%H:%M:%S"), username);
}