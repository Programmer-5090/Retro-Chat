use tokio::{ io::{ AsyncWrite, AsyncWriteExt }, sync::mpsc };
use chrono::Local;
use redis::Commands;

use crate::{ ChatMessage, MessageType, generate_message_id, AppState };

use super::send_notice;

pub(super) async fn handle_regular_message(
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
        ..Default::default()
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
            ..Default::default()
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

pub(super) async fn handle_typing(
    state: &AppState,
    redis_conn: &mut redis::Connection,
    username: &str,
    _out_tx: &mpsc::UnboundedSender<String>,
    input: &str
) {
    let room = if
        let Some(r) = input.strip_prefix("/typing ").and_then(|r| {
            let t = r.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
    {
        r
    } else {
        state.get_last_room(username).await.unwrap_or_else(|| "general".to_string())
    };
    let _: () = redis_conn.set_ex(format!("typing:{}:{}", room, username), "1", 10).unwrap();
    let typing_msg = ChatMessage {
        id: String::new(),
        username: username.to_string(),
        content: String::new(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::TypingNotification,
        room: room.clone(),
        ..Default::default()
    };
    let typing_json = serde_json::to_string(&typing_msg).unwrap();
    state.send_to_room(&room, &typing_json).await;
}

pub(super) async fn handle_image_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    username: &str,
    current_room: &str,
    args: &str
) {
    let parts: Vec<&str> = args.splitn(4, ' ').collect();
    if parts.len() >= 2 {
        let image_url = parts[0].to_string();
        let thumb_url = parts[1].to_string();
        let width: u32 = parts
            .get(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let height: u32 = parts
            .get(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let image_msg = ChatMessage {
            id: generate_message_id(),
            username: username.to_string(),
            content: String::new(),
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            message_type: MessageType::ImageMessage,
            room: current_room.to_string(),
            image_url,
            thumb_url,
            width,
            height,
            ..Default::default()
        };
        let image_json = serde_json::to_string(&image_msg).unwrap();
        let room_id = state.get_or_create_db_room(current_room, username).await;
        sqlx::query(
            "INSERT INTO messages (room_id, username, content, message_type, image_url, thumb_url, width, height) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
            .bind(room_id)
            .bind(username)
            .bind(&image_msg.content)
            .bind("ImageMessage")
            .bind(&image_msg.image_url)
            .bind(&image_msg.thumb_url)
            .bind(image_msg.width as i32)
            .bind(image_msg.height as i32)
            .execute(&state.pool).await
            .ok();
        state.send_to_room(current_room, &image_json).await;
    } else {
        send_notice(writer, "Usage: /image <url> <thumb_url> [width] [height]").await;
    }
}

pub(super) async fn handle_audio_command(
    state: &AppState,
    writer: &mut (impl AsyncWrite + Unpin),
    username: &str,
    current_room: &str,
    args: &str
) {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() >= 2 {
        let audio_url = parts[0].to_string();
        let duration_ms: u32 = parts[1].trim().parse().unwrap_or(0);
        let audio_msg = ChatMessage {
            id: generate_message_id(),
            username: username.to_string(),
            content: String::new(),
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            message_type: MessageType::AudioMessage,
            room: current_room.to_string(),
            is_history: false,
            image_url: String::new(),
            thumb_url: String::new(),
            width: 0,
            height: 0,
            mp3_url: String::new(),
            audio_note_url: audio_url.clone(),
            audio_duration_ms: duration_ms,
        };
        let audio_json = serde_json::to_string(&audio_msg).unwrap();
        let room_id = state.get_or_create_db_room(current_room, username).await;
        sqlx::query(
            "INSERT INTO messages (room_id, username, content, message_type, audio_url, audio_duration_ms) VALUES ($1, $2, $3, $4, $5, $6)"
        )
            .bind(room_id)
            .bind(username)
            .bind(&audio_msg.content)
            .bind("AudioMessage")
            .bind(&audio_url)
            .bind(duration_ms as i32)
            .execute(&state.pool).await
            .ok();
        state.send_to_room(current_room, &audio_json).await;
    } else {
        send_notice(writer, "Usage: /audio <url> <duration_ms>").await;
    }
}
