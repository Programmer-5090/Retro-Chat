use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use chrono::Local;

use crate::ChatMessage;
use crate::message::MessageType;
use super::types::{AMBER, CYAN, FocusPane};

pub fn border_style(pane: FocusPane, focus: FocusPane) -> Style {
    if pane == focus {
        Style::default().fg(CYAN)
    } else {
        Style::default().fg(AMBER)
    }
}

pub fn format_title(username: &str) -> String {
    format!("RETRO CHAT — @{}", username)
}

pub fn format_user_message(msg: &ChatMessage) -> Line<'static> {
    let timestamp = msg.timestamp.chars().take(5).collect::<String>();
    let ts_span = Span::styled(
        format!("[{}] ", timestamp),
        Style::default().fg(AMBER),
    );
    let user_span = Span::styled(
        format!("{} \u{25B6} ", msg.username.clone()),
        Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
    );
    let content_span = Span::styled(
        msg.content.clone(),
        Style::default().fg(AMBER),
    );
    Line::from(vec![ts_span, user_span, content_span])
}

pub fn format_system_message(msg: &ChatMessage) -> Line<'static> {
    let text = format!("*** {} ***", msg.content);
    Line::from(Span::styled(text, Style::default().fg(CYAN)))
}

pub fn make_system_msg(text: &str) -> ChatMessage {
    ChatMessage {
        username: "system".to_string(),
        content: text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
    }
}
