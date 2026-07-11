use ratatui::{
    style::{Color, Modifier, Style},
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
    format!("RETRO CHAT \u{2014} @{}", username)
}

pub fn format_user_message(msg: &ChatMessage, color: Color) -> Vec<Line<'static>> {
    let timestamp = msg.timestamp.chars().take(5).collect::<String>();
    let ts_span = Span::styled(
        format!("[{}] ", timestamp),
        Style::default().fg(color),
    );
    let user_span = Span::styled(
        format!("{} \u{25B6} ", msg.username.clone()),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
    let mut lines: Vec<Line<'static>> = msg.content
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                Line::from(vec![
                    ts_span.clone(),
                    user_span.clone(),
                    Span::styled(line.to_string(), Style::default().fg(color)),
                ])
            } else {
                let indent = " ".repeat(timestamp.len() + msg.username.len() + 5);
                Line::from(vec![
                    Span::styled(indent, Style::default().fg(color)),
                    Span::styled(line.to_string(), Style::default().fg(color)),
                ])
            }
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(vec![ts_span, user_span]));
    }
    lines
}

pub fn format_system_message(msg: &ChatMessage) -> Vec<Line<'static>> {
    if msg.content.is_empty() {
        return vec![Line::from(Span::styled(
            "*** ***".to_string(),
            Style::default().fg(CYAN),
        ))];
    }
    msg.content
        .lines()
        .map(|line| {
            let text = format!("*** {} ***", line);
            Line::from(Span::styled(text, Style::default().fg(CYAN)))
        })
        .collect()
}

pub fn make_system_msg(text: &str) -> ChatMessage {
    ChatMessage {
        id: String::new(),
        username: "system".to_string(),
        content: text.to_string(),
        timestamp: Local::now().format("%H:%M:%S").to_string(),
        message_type: MessageType::SystemNotification,
        room: String::new(),
        is_history: false,
    }
}