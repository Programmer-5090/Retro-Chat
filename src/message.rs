use serde::{ Deserialize, Serialize };
use chrono::Local;
use derive_more::Display;
use rand::Rng;

#[derive(Debug, Clone, Serialize, Deserialize, Display)]
#[display("{username}: {content}")]
pub struct ChatMessage {
    /// Unique id for this message. Empty for messages that never need to be
    /// acknowledged (system notices, room lists, read receipts themselves).
    #[serde(default)]
    pub id: String,
    pub username: String,
    pub content: String,
    pub timestamp: String,
    pub message_type: MessageType,
    #[serde(default)]
    pub room: String,
    /// True when this message is being replayed from history (e.g. right
    /// after joining a room) rather than delivered live. Used by the client
    /// to avoid treating a backlog replay as "new activity" in the room.
    #[serde(default)]
    pub is_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Display)]
pub enum MessageType {
    UserMessage,
    SystemNotification,
    RoomList,
    /// Broadcast by the server on behalf of a user who has just read one or
    /// more messages in a room. `content` is a comma-separated list of
    /// message ids that are now considered read; `username` is the reader.
    ReadReceipt,
    /// Sent by the server to tell clients who is currently online (from Redis
    /// presence keys). `content` is a comma-separated list of usernames.
    PresenceSync,
    /// Broadcast by the server while a user is typing in a room. Receiving
    /// clients show an animated indicator. The sender skips it.
    TypingNotification,
    /// Sent by the server during login to tell the client which room it
    /// should consider active. `content` is the room name.
    SetActiveRoom,
}

/// Generates a short random id to tag a chat message with, so read
/// receipts can reference it later.
pub fn generate_message_id() -> String {
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(12)
        .map(char::from)
        .collect()
}

fn build_msg(text: &str, room: &str) -> String {
    let msg = ChatMessage {
        id: String::new(),
        username: "Server".to_string(),
        content: text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
        room: room.to_string(),
        is_history: false,
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

/// Builds a read-receipt wire message (no trailing newline — intended for
/// `AppState::send_to_room`, which appends its own newline when writing).
/// `reader` has read the messages with the given `ids` in `room`; senders
/// still present in that room will flip those messages to the "read" color.
pub fn build_read_receipt(reader: &str, room: &str, ids: &[String]) -> String {
    let msg = ChatMessage {
        id: String::new(),
        username: reader.to_string(),
        content: ids.join(","),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::ReadReceipt,
        room: room.to_string(),
        is_history: false,
    };
    serde_json::to_string(&msg).unwrap()
}
