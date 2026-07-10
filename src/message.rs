use serde::{ Deserialize, Serialize };
use chrono::Local;
use derive_more::Display;

#[derive(Debug, Clone, Serialize, Deserialize, Display)]
#[display("{username}: {content}")]
pub struct ChatMessage {
    pub username: String,
    pub content: String,
    pub timestamp: String,
    pub message_type: MessageType,
}

#[derive(Debug, Clone, Serialize, Deserialize, Display)]
pub enum MessageType {
    UserMessage,
    SystemNotification,
    RoomList,
}

pub fn build_notice(text: &str) -> String {
    let msg = ChatMessage {
        username: "Server".to_string(),
        content: text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
    };
    let json = serde_json::to_string(&msg).unwrap();
    format!("{}\n", json)
}
