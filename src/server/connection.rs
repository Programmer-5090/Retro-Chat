use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{
    io::{ AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, AsyncBufReadExt },
    sync::mpsc,
    time::interval,
};
use chrono::Local;
use redis::Commands;

use crate::{ ChatMessage, MessageType, AppState };

use super::send_notice;
use super::auth::{ validate_username, check_banned, authenticate_user };
use super::rooms::{
    send_room_list,
    replay_history,
    handle_join_command,
    handle_leave_command,
    handle_switch_command,
};
use super::commands::{
    handle_msg_command,
    handle_read_command,
    handle_mute_command,
    handle_unmute_command,
    handle_ban_command,
    handle_unban_command,
};
use super::messages::{
    handle_regular_message,
    handle_typing,
    handle_image_command,
    handle_audio_command,
};

async fn cleanup_disconnect(state: &AppState, redis_conn: &mut redis::Connection, username: &str) {
    let _: Result<usize, _> = redis_conn.del(format!("presence:{}", username));
    let user_room = state.get_user_room(username).await;
    let _ = redis_conn.del::<_, usize>(format!("typing:{}:{}", user_room, username));

    let subscribed_rooms = state.get_user_subscribed_rooms(username).await;
    for room in &subscribed_rooms {
        let leave_msg = ChatMessage {
            id: String::new(),
            username: username.to_string(),
            content: format!("{} Left the chat", username).to_string(),
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            message_type: MessageType::SystemNotification,
            room: room.clone(),
            ..Default::default()
        };
        let leave_json = serde_json::to_string(&leave_msg).unwrap();
        state.send_to_room(room, &leave_json).await;
    }

    state.leave_all_rooms(username).await;
    state.unregister_sender(username).await;
}

async fn dispatch_command(
    input: &str,
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    redis_conn: &mut redis::Connection,
    username: &str,
    user_role: &str,
    out_tx: &mpsc::UnboundedSender<String>
) {
    if let Some(args) = input.strip_prefix("/join ") {
        handle_join_command(state, writer, username, out_tx, args).await;
    } else if input == "/leave" {
        handle_leave_command(state, writer, username, out_tx).await;
    } else if input.starts_with("/rooms") {
        let rooms = state.list_db_rooms().await;
        if rooms.is_empty() {
            send_notice(writer, "No rooms available.").await;
        } else {
            send_notice(writer, &format!("Available rooms: {}", rooms.join(", "))).await;
        }
    } else if let Some(args) = input.strip_prefix("/msg ") {
        handle_msg_command(state, writer, username, out_tx, args).await;
    } else if let Some(args) = input.strip_prefix("/read ") {
        handle_read_command(state, username, args).await;
    } else if let Some(args) = input.strip_prefix("/mute ") {
        handle_mute_command(state, writer, redis_conn, username, user_role, args).await;
    } else if let Some(args) = input.strip_prefix("/unmute ") {
        handle_unmute_command(state, writer, redis_conn, username, user_role, args).await;
    } else if let Some(args) = input.strip_prefix("/ban ") {
        handle_ban_command(state, writer, username, user_role, args).await;
    } else if let Some(args) = input.strip_prefix("/unban ") {
        handle_unban_command(state, writer, username, user_role, args).await;
    } else if input.starts_with("/typing") {
        handle_typing(state, redis_conn, username, out_tx, input).await;
    } else if let Some(args) = input.strip_prefix("/image ") {
        handle_image_command(state, writer, username, args).await;
    } else if let Some(args) = input.strip_prefix("/audio ") {
        handle_audio_command(state, writer, username, args).await;
    } else if let Some(args) = input.strip_prefix("/switch ") {
        handle_switch_command(state, username, args).await;
    } else if input == "/help" {
        send_notice(
            writer,
            "Commands: /join <room>, /rooms, /msg <user> <text>, /image <url> <thumb_url>, /audio <url> <duration_ms>, /help | Admin: /mute, /unmute, /ban, /unban"
        ).await;
    } else {
        send_notice(writer, "Unknown command. Try /help.").await;
    }
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
        ..Default::default()
    };
    let active_json = serde_json::to_string(&active_msg).unwrap();
    let _ = out_tx.send(active_json);

    let join_msg = ChatMessage {
        id: String::new(),
        username: username.clone(),
        content: format!("{} Joined the chat", username).to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
        room: current_room.clone(),
        ..Default::default()
    };
    let join_json = serde_json::to_string(&join_msg).unwrap();
    state.send_to_room(&current_room, &join_json).await;

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
        ..Default::default()
    };
    let sync_json = serde_json::to_string(&sync_msg).unwrap();
    let _ = out_tx.send(sync_json);

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
                    ..Default::default()
                };
                let t_json = serde_json::to_string(&t_msg).unwrap();
                let _ = out_tx.send(t_json);
            }
        }
    }

    let mut presence_heartbeat = interval(Duration::from_secs(10));
    presence_heartbeat.tick().await;

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
                    dispatch_command(
                        input,
                        &state,
                        &mut writer,
                        &mut redis_conn,
                        &username,
                        &user_role,
                        &out_tx
                    ).await;
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
