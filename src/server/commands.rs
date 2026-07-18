use tokio::{ io::AsyncWrite, sync::mpsc };
use chrono::Local;
use redis::Commands;

use crate::{ ChatMessage, MessageType, build_read_receipt, AppState };

use super::send_notice;

pub(super) async fn handle_msg_command(
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
    super::rooms::replay_history(state, &dm_room, out_tx).await;
    super::rooms::send_room_list(state, username, out_tx).await;

    let dm_msg = ChatMessage {
        id: crate::generate_message_id(),
        username: username.to_string(),
        content: dm_text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::UserMessage,
        room: dm_room.clone(),
        ..Default::default()
    };
    let dm_json = serde_json::to_string(&dm_msg).unwrap();

    sqlx::query(
        "INSERT INTO messages (username, content, message_type, room_id, message_id) VALUES ($1, $2, $3, $4, $5)"
    )
        .bind(&dm_msg.username)
        .bind(&dm_msg.content)
        .bind(dm_msg.message_type.to_string())
        .bind(room_id)
        .bind(&dm_msg.id)
        .execute(&state.pool).await
        .unwrap();

    state.save_room_membership(target, &dm_room).await;

    if let Some(target_tx) = state.get_sender(target).await {
        state.subscribe_room(target, &dm_room, target_tx.clone()).await;
        let whisper = crate::build_notice(
            &format!("DM from {}: '{}'. Check your sidebar to join.", username, dm_text)
        );
        let _ = target_tx.send(whisper);
        super::rooms::send_room_list(state, target, &target_tx).await;
    }

    state.send_to_room(&dm_room, &dm_json).await;
}

pub(super) async fn handle_read_command(state: &AppState, username: &str, input: &str) {
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

pub(super) async fn handle_mute_command(
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

pub(super) async fn handle_unmute_command(
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

pub(super) async fn handle_ban_command(
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

pub(super) async fn handle_unban_command(
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
