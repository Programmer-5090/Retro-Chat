use tokio::{ io::AsyncWrite, sync::mpsc };
use chrono::Local;

use crate::{ ChatMessage, MessageType, dm_display_name, AppState };

use super::send_notice;

pub(super) async fn send_room_list(
    state: &AppState,
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>
) {
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
        ..Default::default()
    };
    let json = serde_json::to_string(&msg).unwrap();
    let _ = out_tx.send(json);
}

pub(super) async fn replay_history(
    state: &AppState,
    room_name: &str,
    out_tx: &mpsc::UnboundedSender<String>
) {
    let room_id: i32 = state.get_or_create_db_room(room_name, "system").await;
    let rows = sqlx
        ::query_as::<
            _,
            (
                String,
                String,
                chrono::DateTime<chrono::Utc>,
                String,
                Option<String>,
                Option<String>,
                Option<i32>,
                Option<i32>,
                Option<String>,
                Option<i32>,
                String,
            )
        >(
            "SELECT username, content, created_at, message_type, image_url, thumb_url, width, height, audio_url, audio_duration_ms, message_id FROM messages WHERE room_id = $1 ORDER BY created_at DESC LIMIT 50"
        )
        .bind(room_id)
        .fetch_all(&state.pool).await
        .unwrap();

    for row in rows.into_iter().rev() {
        let msg = ChatMessage {
            id: row.10,
            username: row.0,
            content: row.1,
            timestamp: row.2.format("%H:%M:%S").to_string(),
            message_type: match row.3.as_str() {
                "UserMessage" => MessageType::UserMessage,
                "ImageMessage" => MessageType::ImageMessage,
                "AudioMessage" => MessageType::AudioMessage,
                _ => MessageType::SystemNotification,
            },
            room: room_name.to_string(),
            is_history: true,
            image_url: row.4.unwrap_or_default(),
            thumb_url: row.5.unwrap_or_default(),
            width: row.6.unwrap_or(0) as u32,
            height: row.7.unwrap_or(0) as u32,
            mp3_url: String::new(),
            audio_note_url: row.8.unwrap_or_default(),
            audio_duration_ms: row.9.unwrap_or(0) as u32,
        };
        let msg_json = serde_json::to_string(&msg).unwrap();
        let _ = out_tx.send(msg_json);
    }
}

pub(super) async fn handle_join_command(
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

    let dm_move = old_room.starts_with("__dm__") || room_name.starts_with("__dm__");
    if !dm_move {
        let join_notice = ChatMessage {
            id: String::new(),
            username: username.to_string(),
            content: format!("{} Joined the room", username).to_string(),
            timestamp: Local::now().format("%H:%M:%S").to_string(),
            message_type: MessageType::SystemNotification,
            room: room_name.to_string(),
            ..Default::default()
        };
        let join_json = serde_json::to_string(&join_notice).unwrap();
        state.send_to_room(room_name, &join_json).await;
    }
}

pub(super) async fn handle_leave_command(
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
        content: format!("{} Left the room", username).to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
        room: current_room.clone(),
        ..Default::default()
    };
    let leave_json = serde_json::to_string(&leave_notice).unwrap();
    state.send_to_room(&current_room, &leave_json).await;

    let fallback = state.get_last_room(username).await.unwrap_or_else(|| "general".to_string());
    state.subscribe_room(username, &fallback, out_tx.clone()).await;
    state.set_active_room(username, &fallback).await;
    send_notice(
        writer,
        &format!(
            "Left '{}'. Now in '{}'.",
            dm_display_name(&current_room, username),
            dm_display_name(&fallback, username)
        )
    ).await;
    replay_history(state, &fallback, out_tx).await;
    send_room_list(state, username, out_tx).await;
}

pub(super) async fn handle_switch_command(state: &AppState, username: &str, input: &str) {
    let room_name = input.trim().to_string();
    if room_name.is_empty() {
        return;
    }
    state.set_active_room(username, &room_name).await;
}
