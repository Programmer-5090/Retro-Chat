use std::f64::consts::TAU;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use chrono::Local;

use crate::ChatMessage;
use crate::message::MessageType;
use super::types::{AMBER, CYAN, FocusPane};

pub fn border_style(pane: FocusPane, focus: FocusPane, pulse_tick: u64) -> Style {
    if pane == focus {
        let phase = ((pulse_tick as f64) * 0.08).sin() * 0.5 + 0.5;
        let g = (180.0 + phase * 75.0) as u8;
        let b = (200.0 + phase * 55.0) as u8;
        Style::default().fg(Color::Rgb(0, g, b))
    } else {
        Style::default().fg(AMBER)
    }
}

pub fn format_gradient_title(username: &str) -> Line<'static> {
    let text = format!("RETRO CHAT \u{2014} @{}", username);
    let len = text.chars().count().max(1);
    let spans: Vec<Span<'static>> = text
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let t = i as f64 / len as f64;
            let r = (127.0 + 128.0 * (t * TAU).sin()) as u8;
            let g = (127.0 + 128.0 * ((t * TAU) + TAU / 3.0).sin()) as u8;
            let b = (127.0 + 128.0 * ((t * TAU) + 2.0 * TAU / 3.0).sin()) as u8;
            Span::styled(c.to_string(), Style::default().fg(Color::Rgb(r, g, b)))
        })
        .collect();
    Line::from(spans)
}

fn highlight_mentions(text: &str, base_color: Color) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < len {
        if chars[i] == '@' && i + 1 < len && chars[i + 1].is_alphabetic() {
            let start = i;
            i += 1;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-') {
                i += 1;
            }
            let mention: String = chars[start..i].iter().collect();
            spans.push(Span::styled(
                mention,
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ));
        } else {
            let start = i;
            while i < len
                && !(chars[i] == '@' && i + 1 < len && chars[i + 1].is_alphabetic())
            {
                i += 1;
            }
            let seg: String = chars[start..i].iter().collect();
            spans.push(Span::styled(seg, Style::default().fg(base_color)));
        }
    }
    spans
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
    let indent = " ".repeat(timestamp.len() + msg.username.len() + 5);
    let indent_span = Span::styled(indent, Style::default().fg(color));
    let mut lines: Vec<Line<'static>> = msg.content
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let content_spans = highlight_mentions(line, color);
            if i == 0 {
                let mut spans = vec![ts_span.clone(), user_span.clone()];
                spans.extend(content_spans);
                Line::from(spans)
            } else {
                let mut spans = vec![indent_span.clone()];
                spans.extend(content_spans);
                Line::from(spans)
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