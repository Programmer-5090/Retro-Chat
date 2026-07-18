use serde::{ Deserialize, Serialize };
use chrono::Local;
use derive_more::Display;
use rand::Rng;

#[derive(Debug, Clone, Serialize, Deserialize, Display, Default)]
#[display("{username}: {content}")]
pub struct ChatMessage {
    #[serde(default)]
    pub id: String,
    pub username: String,
    pub content: String,
    pub timestamp: String,
    pub message_type: MessageType,
    #[serde(default)]
    pub room: String,
    #[serde(default)]
    pub is_history: bool,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub thumb_url: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub mp3_url: String,
    #[serde(default)]
    pub audio_note_url: String,
    #[serde(default)]
    pub audio_duration_ms: u32, 
}

#[derive(Debug, Clone, Serialize, Deserialize, Display, Default, PartialEq)]
pub enum MessageType {
    #[default]
    UserMessage,
    SystemNotification,
    ImageMessage,
    AudioMessage,
    RoomList,
    ReadReceipt,
    PresenceSync,
    TypingNotification,
    SetActiveRoom,
}

/// Generates a short random id to tag a chat message with, so read
/// receipts can reference it later.
pub fn generate_message_id() -> String {
    rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(12)
        .map(char::from)
        .collect()
}

fn build_msg(text: &str, room: &str) -> String {
    let msg = ChatMessage {
        username: "Server".to_string(),
        content: text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
        room: room.to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&msg).unwrap();
    format!("{}\n", json)
}

pub fn build_notice(text: &str) -> String {
    build_msg(text, "")
}

pub fn build_room_notice(text: &str, room: &str) -> String {
    build_msg(text, room)
}

/// Returns a display-friendly name for a room. DM rooms like
/// `__dm__alice_bob` are shown as the "other" user's name from `username`'s
/// perspective (e.g. "bob" for alice, "alice" for bob). Non-DM rooms are
/// returned as-is.
pub fn dm_display_name<'a>(room: &'a str, username: &str) -> &'a str {
    if let Some(rest) = room.strip_prefix("__dm__") {
        let mut parts: Vec<&str> = rest.split('_').collect();
        if parts.len() == 2 {
            parts.retain(|p| !p.is_empty());
            if parts.len() == 2 {
                return if parts[0] == username { parts[1] } else { parts[0] };
            }
        }
    }
    room
}

pub fn build_read_receipt(reader: &str, room: &str, ids: &[String]) -> String {
    let msg = ChatMessage {
        username: reader.to_string(),
        content: ids.join(","),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::ReadReceipt,
        room: room.to_string(),
        ..Default::default()
    };
    serde_json::to_string(&msg).unwrap()
}
