use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{ Color, Modifier, Style };
use ratatui::symbols::border;
use ratatui::text::Line;
use ratatui::widgets::{ Block, Borders, Clear, List, ListItem, Paragraph, Sparkline, Wrap };
use ratatui_image::Image as RatatuiImage;

use super::app::App;
use super::format::{
    border_style,
    format_title,
    format_system_message,
    format_user_message,
    format_image_header,
    format_audio_message,
    format_spectrum_bars,
    format_idle_waveform,
    username_color,
};
use super::types::{ FocusPane, AnimationKind };
use crate::message::{ dm_display_name, MessageType };

pub(crate) fn render_title_bar(app: &App, f: &mut Frame, area: Rect) {
    let pulse = |c: Color| -> Color {
        if let Color::Rgb(r, g, b) = c {
            let f = 0.6 + 0.4 * ((app.ui.pulse_tick as f64) * 0.04).sin();
            Color::Rgb(((r as f64) * f) as u8, ((g as f64) * f) as u8, ((b as f64) * f) as u8)
        } else {
            c
        }
    };
    let title = format_title(&app.username, pulse(app.ui.theme.primary));
    let widget = Paragraph::new(title)
        .style(Style::default())
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}

pub(crate) fn render_sidebar(app: &mut App, f: &mut Frame, area: Rect) {
    app.ui.sidebar_area = area;

    let max_select = app.rooms.len().saturating_sub(1);
    match app.ui.sidebar_state.selected() {
        Some(i) if i > max_select => {
            app.ui.sidebar_state.select(Some(max_select));
        }
        None if !app.rooms.is_empty() => {
            app.ui.sidebar_state.select(Some(0));
        }
        _ => {}
    }

    let items: Vec<ListItem> = app.rooms
        .iter()
        .map(|room| {
            let has_unread = app.read.unread_rooms.contains(room);
            let active = room == &app.current_room;
            let prefix = if has_unread { "\u{25CF} " } else { "  " };
            let display = format!("{}{}", prefix, dm_display_name(room, &app.username));
            let style = if active {
                Style::default().fg(app.ui.theme.accent)
            } else if has_unread {
                Style::default().fg(app.ui.theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.ui.theme.primary)
            };
            ListItem::new(ratatui::text::Line::from(ratatui::text::Span::styled(display, style)))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(
            border_style(FocusPane::Sidebar, app.input.focus, app.ui.pulse_tick, &app.ui.theme)
        )
        .title(" Messages ");

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(app.ui.theme.primary)
                .fg(app.ui.theme.bg)
                .add_modifier(Modifier::BOLD)
        )
        .highlight_symbol("\u{25B6} ");

    f.render_stateful_widget(list, area, &mut app.ui.sidebar_state);
}

pub(crate) fn render_status_bar(app: &App, f: &mut Frame, area: Rect) {
    let spark_data: Vec<u64> = app.ui.sparkline_data
        .iter()
        .map(|v| *v as u64)
        .collect();
    let spark = Sparkline::default()
        .data(&spark_data)
        .style(Style::default().fg(app.ui.theme.primary).bg(app.ui.theme.bg));
    let spark_area = Rect {
        x: area.x + area.width.saturating_sub(41),
        y: area.y,
        width: (40).min(area.width.saturating_sub(1)),
        height: 1,
    };
    let unread_str = if app.read.unread_rooms.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = app.read.unread_rooms
            .iter()
            .filter(|r| *r != &app.current_room)
            .map(|r| dm_display_name(r, &app.username).to_string())
            .collect();
        if names.is_empty() {
            String::new()
        } else {
            format!(" \u{25CF}{} ", names.join(" \u{25CF}"))
        }
    };
    let typing_text = {
        let names: Vec<&str> = app.typing.typing_users
            .iter()
            .filter(|(_, (r, _))| r == &app.current_room || r.starts_with("__dm__"))
            .map(|(u, _)| u.as_str())
            .collect();
        if names.is_empty() {
            String::new()
        } else if names.len() == 1 {
            let dots = match (app.ui.pulse_tick / 10) % 4 {
                0 => ".",
                1 => "..",
                2 => "...",
                _ => "",
            };
            format!(" {} is typing{} ", names[0], dots)
        } else if names.len() <= 3 {
            format!(" {} and others are typing... ", names.join(", "))
        } else {
            format!(" Several people are typing... ")
        }
    };
    let label = if app.audio.is_recording {
        let elapsed = app.audio.record_start.map(|s| s.elapsed().as_secs()).unwrap_or(0);
        let dots = match (app.ui.pulse_tick / 8) % 4 {
            0 => ".",
            1 => "..",
            2 => "...",
            _ => "",
        };
        format!(
            " \u{25CF} REC{}  {:02}:{:02}  (type /record to stop)",
            dots,
            elapsed / 60,
            elapsed % 60
        )
    } else if !typing_text.is_empty() {
        typing_text
    } else if !unread_str.is_empty() {
        format!(" \u{2191}\u{2193}\u{21b5}:switch  Tab:focus {}", unread_str)
    } else {
        " \u{2191}\u{2193}\u{21b5}:switch  Tab:focus  /help:commands".to_string()
    };
    let status = if app.audio.is_recording {
        Paragraph::new(label).style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        Paragraph::new(label).style(Style::default().fg(Color::DarkGray))
    };
    f.render_widget(status, area);
    if area.width >= 42 {
        f.render_widget(spark, spark_area);
    }
}

pub(crate) fn render_help_popup(app: &mut App, f: &mut Frame, area: Rect) {
    let mut items: Vec<ListItem> = super::commands::HELP_COMMANDS
        .iter()
        .map(|(cmd, desc, _)| {
            let line = format!("{:<18}{}", cmd, desc);
            ListItem::new(
                ratatui::text::Line::from(
                    ratatui::text::Span::styled(line, Style::default().fg(app.ui.theme.primary))
                )
            )
        })
        .collect();

    let keybind_lines = [
        ratatui::text::Line::from(""),
        ratatui::text::Line::from(
            vec![
                ratatui::text::Span::styled(
                    "Ctrl+A",
                    Style::default().fg(app.ui.theme.accent).add_modifier(Modifier::BOLD)
                ),
                ratatui::text::Span::styled(
                    " switch animation",
                    Style::default().fg(app.ui.theme.primary)
                )
            ]
        ),
        ratatui::text::Line::from(
            vec![
                ratatui::text::Span::styled(
                    "Ctrl+T",
                    Style::default().fg(app.ui.theme.accent).add_modifier(Modifier::BOLD)
                ),
                ratatui::text::Span::styled(
                    " switch theme",
                    Style::default().fg(app.ui.theme.primary)
                )
            ]
        ),
    ];
    for line in &keybind_lines {
        items.push(ListItem::new(line.clone()));
    }

    let popup_w = (44u16).min(area.width.saturating_sub(2));
    let popup_h = (
        (super::commands::HELP_COMMANDS.len() as u16) +
        (keybind_lines.len() as u16) +
        2
    ).min(area.height.saturating_sub(2));
    let popup_area = Rect {
        x: area.x + area.width.saturating_sub(popup_w) / 2,
        y: area.y + area.height.saturating_sub(popup_h) / 2,
        width: popup_w,
        height: popup_h,
    };
    app.input.help_area = popup_area;

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(app.ui.theme.accent))
        .style(Style::default().bg(app.ui.theme.bg))
        .title(" Commands ")
        .title_bottom(" \u{2191}\u{2193}\u{21b5} or click \u{2022} Esc to close ");

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(app.ui.theme.primary)
                .fg(app.ui.theme.bg)
                .add_modifier(Modifier::BOLD)
        )
        .highlight_symbol("\u{25B6} ");

    f.render_stateful_widget(list, popup_area, &mut app.input.help_state);
}

pub(crate) fn render_input(app: &mut App, f: &mut Frame, area: Rect) {
    if app.input.focus == FocusPane::Input {
        app.input.textarea.set_cursor_style(Style::default().bg(app.ui.theme.accent));
    } else {
        app.input.textarea.set_cursor_style(Style::default());
    }
    app.input.textarea.set_block(
        Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(
                border_style(FocusPane::Input, app.input.focus, app.ui.pulse_tick, &app.ui.theme)
            )
            .title(" Message ")
    );
    f.render_widget(&app.input.textarea, area);
}

pub(crate) fn render_messages(app: &mut App, f: &mut Frame, area: Rect) {
    app.ui.messages_area = area;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(
            border_style(FocusPane::Messages, app.input.focus, app.ui.pulse_tick, &app.ui.theme)
        )
        .title(format!(" #{} ", dm_display_name(&app.current_room, &app.username)));
    let content_area = block.inner(area);
    f.render_widget(block, area);

    if content_area.height == 0 || content_area.width == 0 {
        return;
    }

    let room_msgs: Vec<_> = app.messages_for_room(&app.current_room).cloned().collect();
    if room_msgs.is_empty() {
        return;
    }

    let content_width = content_area.width as usize;
    let mut msg_heights: Vec<u16> = Vec::with_capacity(room_msgs.len());
    let mut total_height = 0u16;
    for (msg, _) in &room_msgs {
        let h = app.message_line_height(msg, content_width as u16);
        msg_heights.push(h);
        total_height = total_height.saturating_add(h);
    }

    let visible_height = content_area.height;
    let max_scroll = total_height.saturating_sub(visible_height);
    let render_scroll = app.scroll_offset.min(max_scroll);

    let vis_start = total_height.saturating_sub(render_scroll + visible_height);
    let vis_end = total_height.saturating_sub(render_scroll);

    let mut current_y = 0u16;
    for (i, (msg, _)) in room_msgs.iter().enumerate() {
        let h = msg_heights[i];
        let msg_top = current_y;
        let msg_bottom = current_y.saturating_add(h);

        if msg_top >= vis_start && msg_top < vis_end {
            let screen_y = content_area.y + (msg_top - vis_start);
            let max_h = content_area.height.saturating_sub(screen_y - content_area.y);
            let msg_area = Rect {
                x: content_area.x,
                y: screen_y,
                width: content_area.width,
                height: h.min(max_h),
            };

            match msg.message_type {
                MessageType::UserMessage => {
                    let color = if msg.username == app.username {
                        if app.read.read_message_ids.contains(&msg.id) {
                            app.ui.theme.success
                        } else {
                            app.ui.theme.primary
                        }
                    } else {
                        app.ui.theme.secondary
                    };
                    let dot_color = (
                        msg.username != app.username && app.online_users.contains(&msg.username)
                    ).then(|| username_color(&msg.username));
                    let lines = format_user_message(msg, color, app.ui.theme.accent, dot_color);
                    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
                    f.render_widget(para, msg_area);
                }
                MessageType::SystemNotification => {
                    let lines = format_system_message(msg, app.ui.theme.accent);
                    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
                    f.render_widget(para, msg_area);
                }
                MessageType::ImageMessage => {
                    let color = if msg.username == app.username {
                        app.ui.theme.primary
                    } else {
                        app.ui.theme.secondary
                    };
                    let header_lines = format_image_header(msg, color);
                    let header_area = Rect { height: (1).min(msg_area.height), ..msg_area };
                    let header_para = Paragraph::new(header_lines);
                    f.render_widget(header_para, header_area);

                    let img_area = Rect {
                        y: msg_area.y + (1).min(msg_area.height),
                        height: msg_area.height.saturating_sub(1),
                        ..msg_area
                    };

                    if let Some(protocol) = app.images.image_cache.get(&msg.id) {
                        let img_widget = RatatuiImage::new(protocol);
                        f.render_widget(img_widget, img_area);
                    } else {
                        if !app.images.inflight_images.contains(&msg.id) && !msg.id.is_empty() {
                            app.images.inflight_images.insert(msg.id.clone());
                            super::image::spawn_image_fetch(
                                app,
                                msg.id.clone(),
                                msg.thumb_url.clone()
                            );
                        }
                        let placeholder = Paragraph::new(
                            Line::from(
                                ratatui::text::Span::styled(
                                    "  \u{2593}\u{2593} image \u{2593}\u{2593}",
                                    Style::default().fg(app.ui.theme.accent)
                                )
                            )
                        );
                        f.render_widget(placeholder, img_area);
                    }
                }
                MessageType::AudioMessage => {
                    let color = if msg.username == app.username {
                        app.ui.theme.primary
                    } else {
                        app.ui.theme.secondary
                    };
                    let is_playing = app.audio.playing_audio.as_deref() == Some(&msg.id);
                    let lines = format_audio_message(msg, color, is_playing);

                    let text_area = Rect { height: (1).min(msg_area.height), ..msg_area };
                    let para = Paragraph::new(lines);
                    f.render_widget(para, text_area);

                    if is_playing && !app.audio.live_spectrum.is_empty() {
                        let wave_area = Rect {
                            y: msg_area.y + 1,
                            height: msg_area.height.saturating_sub(1),
                            ..msg_area
                        };
                        if wave_area.height > 0 && wave_area.width > 0 {
                            let bars = format_spectrum_bars(
                                &app.audio.live_spectrum,
                                wave_area.width as usize,
                                wave_area.height,
                                Color::Rgb(90, 130, 240),
                                Color::Rgb(230, 60, 150),
                                Color::Rgb(90, 220, 140)
                            );
                            f.render_widget(Paragraph::new(bars), wave_area);
                        }
                    } else if msg_area.height >= 2 {
                        let idle_area = Rect {
                            y: msg_area.y + 1,
                            height: 1,
                            ..msg_area
                        };
                        let muted = Color::Rgb(80, 80, 100);
                        let idle_line = format_idle_waveform(&msg.id, idle_area.width as usize, muted);
                        f.render_widget(Paragraph::new(idle_line), idle_area);
                    }
                }
                _ => {}
            }
        }

        current_y = msg_bottom;
    }
}

pub(crate) fn render_animations(app: &mut App, f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(app.ui.theme.primary));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let t = app.ui.start_time.elapsed().as_secs_f64();

    let square_area = {
        let box_w = (inner.height * 2).min(inner.width);
        let x_off = inner.width.saturating_sub(box_w) / 2;
        Rect {
            x: inner.x + x_off,
            y: inner.y,
            width: box_w,
            height: inner.height,
        }
    };

    match app.ui.anim_kind {
        AnimationKind::Cube => app.ui.cube.render(f, square_area, t),
        AnimationKind::Torus => app.ui.torus.render(f, square_area, t),
        AnimationKind::MatrixRain => app.ui.matrix_rain.render(f, inner, t),
        AnimationKind::Starfield => app.ui.starfield.render(f, inner, t),
        AnimationKind::Sand => app.ui.sand.render(f, inner, t),
    }
}
