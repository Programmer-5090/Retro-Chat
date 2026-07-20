use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{ Duration, Instant };

use tokio::io::AsyncWriteExt;

use crate::ChatMessage;
use crate::message::{ MessageType, generate_message_id };
use super::app::App;
use super::format::make_system_msg;

pub(crate) fn ingest_msg(app: &mut App, mut msg: ChatMessage, read: bool) {
    app.message_times.push_back(Instant::now());
    if app.message_times.len() > 200 {
        app.message_times.pop_front();
    }

    let was_unread = !read
        && !msg.room.is_empty()
        && msg.room != app.current_room
        && msg.message_type != MessageType::SystemNotification;
    if was_unread {
        app.read.unread_rooms.insert(msg.room.clone());
    }

    if msg.message_type == MessageType::SystemNotification && !msg.is_history {
        if msg.id.is_empty() {
            msg.id = generate_message_id();
        }
        app.system_expiry.insert(msg.id.clone(), Instant::now() + Duration::from_secs(3));
    }

    app.messages.push((msg, was_unread));
    app.dirty = true;
    let visible = app.ui.messages_area.height.saturating_sub(2) as usize;
    let visible = if visible == 0 { 20 } else { visible };
    clamp_scroll(app, visible);
}

pub(crate) fn clear_room_read_state(app: &mut App, room: &str) {
    for pair in &mut app.messages {
        let same_room = pair.0.room == room || (pair.0.room.is_empty() && room == app.current_room);
        if same_room {
            pair.1 = false;
        }
    }
    app.read.unread_rooms.remove(room);
}

pub(crate) fn collect_unread_ids(app: &App, room: &str) -> Vec<String> {
    app.messages
        .iter()
        .filter(|(msg, _)| {
            msg.username != app.username &&
                !msg.id.is_empty() &&
                !app.read.read_message_ids.contains(&msg.id) &&
                (msg.room == room || (msg.room.is_empty() && room == app.current_room))
        })
        .map(|(msg, _)| msg.id.clone())
        .collect()
}

pub(crate) async fn mark_all_read(app: &mut App, room: &str) {
    let ids = collect_unread_ids(app, room);
    clear_room_read_state(app, room);
    send_read_receipt(app, room, ids).await;
}

pub(crate) async fn send_read_receipt(app: &App, room: &str, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    let wire = format!("/read {} {}\n", room, ids.join(","));
    let _ = app.writer.lock().await.write_all(wire.as_bytes()).await;
}

pub(crate) fn messages_for_room<'a>(
    app: &'a App,
    room: &'a str
) -> impl Iterator<Item = &'a (ChatMessage, bool)> {
    app.messages
        .iter()
        .filter(move |(msg, _)| {
            msg.room == room || (msg.room.is_empty() && room == app.current_room)
        })
}

pub(crate) fn clamp_scroll(app: &mut App, visible_height: usize) {
    let content_width = app.ui.messages_area.width.saturating_sub(2) as usize;
    let total = total_content_height(app, content_width as u16);
    let max = total.saturating_sub(visible_height as u16) as u16;
    app.scroll_offset = app.scroll_offset.min(max);
}

pub(crate) fn cleanup_sys_messages(app: &mut App) {
    let now = Instant::now();
    let expired: Vec<String> = app.system_expiry
        .iter()
        .filter(|(_, exp)| now >= **exp)
        .map(|(id, _)| id.clone())
        .collect();
    if expired.is_empty() {
        return;
    }
    let expired_set: HashSet<String> = expired.iter().cloned().collect();
    app.messages.retain(|(msg, _)| !expired_set.contains(&msg.id));
    for id in &expired {
        app.system_expiry.remove(id);
    }
    app.dirty = true;
    let visible = app.ui.messages_area.height.saturating_sub(2) as usize;
    let visible = if visible == 0 { 20 } else { visible };
    clamp_scroll(app, visible);
}

pub(crate) fn message_id_at_row(app: &App, screen_row: u16) -> Option<String> {
    let content_width = app.ui.messages_area.width.saturating_sub(2);
    if content_width == 0 {
        return None;
    }

    let content_y = app.ui.messages_area.y + 1;
    if screen_row < content_y {
        return None;
    }
    let row_offset = screen_row - content_y;

    let room_msgs: Vec<_> = app.messages_for_room(&app.current_room).cloned().collect();
    if room_msgs.is_empty() {
        return None;
    }

    let mut msg_heights: Vec<u16> = Vec::with_capacity(room_msgs.len());
    let mut total_height = 0u16;
    for (msg, _) in &room_msgs {
        let h = message_line_height(app, msg, content_width);
        msg_heights.push(h);
        total_height = total_height.saturating_add(h);
    }

    let visible_height = app.ui.messages_area.height.saturating_sub(2);
    let max_scroll = total_height.saturating_sub(visible_height);
    let render_scroll = app.scroll_offset.min(max_scroll);
    let vis_start = total_height.saturating_sub(render_scroll + visible_height);

    let mut current_y = 0u16;
    for (i, (msg, _)) in room_msgs.iter().enumerate() {
        let h = msg_heights[i];
        let msg_top = current_y;
        let msg_bottom = current_y.saturating_add(h);

        if msg_top >= vis_start && msg_top < vis_start + visible_height {
            let rel_row = msg_top - vis_start;
            if row_offset >= rel_row && row_offset < rel_row + h {
                return Some(msg.id.clone());
            }
        }

        current_y = msg_bottom;
    }
    None
}

pub(crate) fn message_line_height(app: &App, msg: &ChatMessage, content_width: u16) -> u16 {
    match msg.message_type {
        MessageType::ImageMessage => {
            let header_lines = 1u16;
            header_lines + app.images.image_cell_height
        }
        MessageType::AudioMessage => {
            let is_playing = app.audio.playing_audio.as_deref() == Some(msg.id.as_str());
            if is_playing {
                4
            } else {
                2
            }
        }
        MessageType::SystemNotification => {
            if msg.content.is_empty() {
                return 1;
            }
            let wrap_width = content_width as usize;
            if wrap_width == 0 {
                return 1;
            }
            let overhead = "*** ".len() + " ***".len();
            let mut total = 0u16;
            for line in msg.content.lines() {
                let line_chars = line.chars().count() + overhead;
                let rows = ((line_chars as u16) + (wrap_width as u16) - 1) / (wrap_width as u16);
                total += rows.max(1);
            }
            total.max(1)
        }
        MessageType::UserMessage => {
            let ts_len = 5usize;
            let user_len = msg.username.len();
            let overhead = ts_len + user_len + 5;
            let wrap_width = content_width as usize;
            if wrap_width == 0 {
                return 1;
            }
            let lines: Vec<&str> = msg.content.lines().collect();
            if lines.is_empty() {
                return 1;
            }
            let mut total = 0u16;
            for line in &lines {
                let line_chars = line.chars().count();
                if line_chars == 0 {
                    total += 1;
                } else {
                    let first_wrap = wrap_width.saturating_sub(overhead);
                    if first_wrap == 0 {
                        total += 1;
                    } else if line_chars <= first_wrap {
                        total += 1;
                    } else {
                        total += 1;
                        let remaining = line_chars - first_wrap;
                        total +=
                            ((remaining as u16) + (wrap_width as u16) - 1) / (wrap_width as u16);
                    }
                }
            }
            total.max(1)
        }
        _ => 1,
    }
}

pub(crate) fn total_content_height(app: &App, content_width: u16) -> u16 {
    messages_for_room(app, &app.current_room.clone())
        .map(|(msg, _)| message_line_height(app, msg, content_width))
        .sum()
}

pub(crate) async fn handle_server_message(app: &mut App, line: &str) {
    if line == "__CONN_CLOSED__" {
        super::audio::stop_playback(app);
        ingest_msg(app, make_system_msg("Connection closed by server"), true);
        app.should_quit = true;
        return;
    }

    if let Some(id) = line.strip_prefix("__PLAYBACK_DONE__:") {
        if app.audio.playing_audio.as_deref() == Some(id) {
            if let Some(flag) = app.audio.spectrum_stop.take() {
                flag.store(true, Ordering::Relaxed);
            }
            app.audio.live_spectrum.clear();
            app.audio.playing_audio = None;
            ingest_msg(app, make_system_msg("Playback finished."), true);
        }
        return;
    }

    if let Ok(msg) = serde_json::from_str::<ChatMessage>(line) {
        match msg.message_type {
            MessageType::RoomList => {
                app.rooms = msg.content
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if app.rooms.is_empty() {
                    app.rooms.push("general".to_string());
                }
                if !app.rooms.iter().any(|r| r == &app.current_room) {
                    app.current_room = app.rooms[0].clone();
                }
            }
            MessageType::SetActiveRoom => {
                let room = msg.content.trim().to_string();
                if !room.is_empty() {
                    app.current_room = room.clone();
                    mark_all_read(app, &room).await;
                }
            }
            MessageType::ReadReceipt => {
                for id in msg.content
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty()) {
                    app.read.read_message_ids.insert(id.to_string());
                }
            }
            MessageType::UserMessage => {
                let is_current_room = msg.room.is_empty() || msg.room == app.current_room;
                let room = msg.room.clone();
                let is_history = msg.is_history;
                let ack_id = if is_current_room && !is_history && msg.username != app.username {
                    msg.id.clone()
                } else {
                    String::new()
                };
                if is_history && msg.username == app.username && !msg.id.is_empty() {
                    app.read.read_message_ids.insert(msg.id.clone());
                }
                ingest_msg(app, msg, is_current_room || is_history);
                if is_current_room && !is_history {
                    clear_room_read_state(app, &room);
                    if !ack_id.is_empty() {
                        let wire = format!("/read {} {}\n", room, ack_id);
                        let _ = app.writer.lock().await.write_all(wire.as_bytes()).await;
                    }
                }
            }
            MessageType::ImageMessage => {
                let is_current_room = msg.room.is_empty() || msg.room == app.current_room;
                let room = msg.room.clone();
                let is_history = msg.is_history;
                if is_history && msg.username == app.username && !msg.id.is_empty() {
                    app.read.read_message_ids.insert(msg.id.clone());
                }
                ingest_msg(app, msg, is_current_room || is_history);
                if is_current_room && !is_history {
                    clear_room_read_state(app, &room);
                }
            }
            MessageType::AudioMessage => {
                let is_current_room = msg.room.is_empty() || msg.room == app.current_room;
                let room = msg.room.clone();
                let is_history = msg.is_history;
                if is_history && msg.username == app.username && !msg.id.is_empty() {
                    app.read.read_message_ids.insert(msg.id.clone());
                }
                ingest_msg(app, msg, is_current_room || is_history);
                if is_current_room && !is_history {
                    clear_room_read_state(app, &room);
                }
            }
            MessageType::TypingNotification => {
                if msg.username != app.username {
                    let room = if msg.room.is_empty() {
                        app.current_room.clone()
                    } else {
                        msg.room.clone()
                    };
                    app.typing.typing_users.insert(msg.username.clone(), (room, Instant::now()));
                }
            }
            MessageType::PresenceSync => {
                for u in msg.content
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty()) {
                    app.online_users.insert(u.to_string());
                }
            }
            MessageType::SystemNotification => {
                let read = msg.is_history || msg.room.is_empty() || msg.room == app.current_room;
                if !msg.is_history {
                    if
                        msg.content.ends_with(" Joined the chat") ||
                        msg.content.ends_with(" Joined the room")
                    {
                        app.online_users.insert(msg.username.clone());
                    } else if msg.content.ends_with(" Left the chat") {
                        app.online_users.remove(&msg.username);
                    }
                }
                ingest_msg(app, msg, read);
            }
        }
    } else if !line.is_empty() {
        ingest_msg(app, make_system_msg(line), true);
    }
}
