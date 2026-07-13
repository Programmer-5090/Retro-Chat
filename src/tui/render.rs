use std::collections::hash_map::DefaultHasher;
use std::hash::{ Hash, Hasher };

use ratatui::{ style::{ Color, Modifier, Style }, text::{ Line, Span } };
use chrono::Local;

use crate::ChatMessage;
use crate::message::MessageType;
use super::types::{ FocusPane, Theme };

pub fn border_style(pane: FocusPane, focus: FocusPane, pulse_tick: u64, theme: &Theme) -> Style {
    if pane == focus {
        let phase = ((pulse_tick as f64) * 0.08).sin() * 0.5 + 0.5;
        let factor = 0.7 + phase * 0.3;
        if let Color::Rgb(r, g, b) = theme.accent {
            let nr = ((r as f64) * factor) as u8;
            let ng = ((g as f64) * factor) as u8;
            let nb = ((b as f64) * factor) as u8;
            Style::default().fg(Color::Rgb(nr, ng, nb))
        } else {
            Style::default().fg(theme.accent)
        }
    } else {
        Style::default().fg(theme.primary)
    }
}

pub fn username_color(username: &str) -> Color {
    let mut hasher = DefaultHasher::new();
    username.hash(&mut hasher);
    let h = (hasher.finish() as f64) / (u64::MAX as f64);
    let hue = h * 360.0;
    let (r, g, b) = hsl_to_rgb(hue, 0.8, 0.6);
    Color::Rgb(r, g, b)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match (h as i32) % 360 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (((r1 + m) * 255.0) as u8, ((g1 + m) * 255.0) as u8, ((b1 + m) * 255.0) as u8)
}

pub fn format_title(username: &str, color: Color) -> Line<'static> {
    let text = format!("@{}", username);
    let line = format!(
        "{}{}{}",
        "\u{28FF}".repeat(4),
        text,
        "\u{28FF}".repeat(4),
    );
    Line::from(Span::styled(line, Style::default().fg(color)))
}

fn highlight_mentions(text: &str, base_color: Color, mention_color: Color) -> Vec<Span<'static>> {
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
            spans.push(
                Span::styled(
                    mention,
                    Style::default().fg(mention_color).add_modifier(Modifier::BOLD)
                )
            );
        } else {
            let start = i;
            while i < len && !(chars[i] == '@' && i + 1 < len && chars[i + 1].is_alphabetic()) {
                i += 1;
            }
            let seg: String = chars[start..i].iter().collect();
            spans.push(Span::styled(seg, Style::default().fg(base_color)));
        }
    }
    spans
}

pub fn format_user_message(
    msg: &ChatMessage,
    color: Color,
    mention_color: Color,
    dot_color: Option<Color>
) -> Vec<Line<'static>> {
    let timestamp = msg.timestamp.chars().take(5).collect::<String>();
    let ts_span = Span::styled(format!("[{}] ", timestamp), Style::default().fg(color));
    let dot_span = dot_color.map(|dc| { Span::styled("\u{25CF} ", Style::default().fg(dc)) });
    let user_span = Span::styled(
        format!("{} \u{25B6} ", msg.username.clone()),
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    );
    let dot_extra = dot_span
        .as_ref()
        .map(|_| 2u16)
        .unwrap_or(0);
    let indent = " ".repeat(timestamp.len() + msg.username.len() + 5 + (dot_extra as usize));
    let indent_span = Span::styled(indent, Style::default().fg(color));
    let mut lines: Vec<Line<'static>> = msg.content
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let content_spans = highlight_mentions(line, color, mention_color);
            if i == 0 {
                let mut spans = vec![ts_span.clone()];
                if let Some(ref dot) = dot_span {
                    spans.push(dot.clone());
                }
                spans.push(user_span.clone());
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
        let mut spans = vec![ts_span];
        if let Some(dot) = dot_span {
            spans.push(dot);
        }
        spans.push(user_span);
        lines.push(Line::from(spans));
    }
    lines
}

pub fn format_system_message(msg: &ChatMessage, color: Color) -> Vec<Line<'static>> {
    if msg.content.is_empty() {
        return vec![Line::from(Span::styled("*** ***".to_string(), Style::default().fg(color)))];
    }
    msg.content
        .lines()
        .map(|line| {
            let text = format!("*** {} ***", line);
            Line::from(Span::styled(text, Style::default().fg(color)))
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
        image_url: String::new(),
        thumb_url: String::new(),
        width: 0,
        height: 0,
    }
}

pub fn format_image_message(
    msg: &ChatMessage,
    color: Color,
    mention_color: Color,
) -> Vec<Line<'static>> {
    let timestamp = msg.timestamp.chars().take(5).collect::<String>();
    let ts_span = Span::styled(format!("[{}] ", timestamp), Style::default().fg(color));
    let user_span = Span::styled(
        format!("{} \u{25B6} ", msg.username.clone()),
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    );
    let label = if msg.content.is_empty() {
        "[image]".to_string()
    } else {
        format!("[image: {}]", msg.content)
    };
    let img_span = Span::styled(
        label,
        Style::default().fg(mention_color).add_modifier(Modifier::BOLD)
    );
    let dim = if msg.width > 0 && msg.height > 0 {
        format!(" ({}x{})", msg.width, msg.height)
    } else {
        String::new()
    };
    let dim_span = Span::styled(dim, Style::default().fg(color));
    vec![Line::from(vec![ts_span, user_span, img_span, dim_span])]
}