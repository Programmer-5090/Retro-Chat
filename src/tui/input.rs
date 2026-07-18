use std::time::Instant;

use crossterm::event::{
    self, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::Style;
use tokio::io::AsyncWriteExt;

use super::app::App;
use super::types::FocusPane;

pub(crate) async fn handle_key(app: &mut App, key: event::KeyEvent) {
    use crossterm::event::{ KeyCode, KeyModifiers };

    if app.input.show_help {
        handle_help_key(app, key).await;
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.should_quit = true;
            return;
        }
        KeyCode::Tab => {
            tab_forward(app);
            return;
        }
        KeyCode::BackTab => {
            tab_backward(app);
            return;
        }
        KeyCode::F(1) => {
            app.input.focus = FocusPane::Sidebar;
            return;
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.ui.theme_idx = (app.ui.theme_idx + 1) % super::types::THEMES.len();
            app.ui.theme = super::types::THEMES[app.ui.theme_idx];
            app.ui.cube.color = app.ui.theme.secondary;
            app.ui.matrix_rain.color = app.ui.theme.secondary;
            app.ui.starfield.color = app.ui.theme.secondary;
            app.ui.torus.color = app.ui.theme.secondary;
            app.ui.sand.color = app.ui.theme.secondary;
            app.input.textarea.set_style(Style::default().fg(app.ui.theme.primary).bg(app.ui.theme.bg));
            app.input.textarea.set_cursor_style(Style::default().bg(app.ui.theme.primary));
            return;
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.ui.anim_kind = app.ui.anim_kind.next();
            return;
        }
        _ => {}
    }

    match app.input.focus {
        FocusPane::Input =>
            match key.code {
                KeyCode::Enter => {
                    let text = app.input.textarea.lines().first().cloned().unwrap_or_default();
                    app.input.textarea.select_all();
                    app.input.textarea.cut();
                    app.last_keypress = Instant::now();
                    super::commands::send_or_command(app, text).await;
                }
                KeyCode::Char(_) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let current_len = app.input.textarea
                        .lines()
                        .first()
                        .map(|l| l.len())
                        .unwrap_or(0);
                    if current_len < 500 {
                        app.input.textarea.input(key);
                        app.last_keypress = Instant::now();
                    }
                }
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End => {
                    app.input.textarea.input(key);
                }
                _ => {}
            }
        FocusPane::Messages =>
            match key.code {
                KeyCode::Up => {
                    let visible = app.ui.messages_area.height.saturating_sub(2) as usize;
                    scroll_up(app, visible);
                }
                KeyCode::Down => {
                    scroll_down(app);
                }
                KeyCode::Enter | KeyCode::Char('p') => {
                    super::audio::toggle_play_audio(app);
                }
                _ => {}
            }
        FocusPane::Sidebar => {
            let max = app.rooms.len().saturating_sub(1);
            match key.code {
                KeyCode::Up => {
                    let i = app.ui.sidebar_state.selected().unwrap_or(0);
                    app.ui.sidebar_state.select(Some(i.saturating_sub(1)));
                }
                KeyCode::Down => {
                    let i = app.ui.sidebar_state.selected().unwrap_or(0);
                    app.ui.sidebar_state.select(Some((i + 1).min(max)));
                }
                KeyCode::Enter => {
                    if let Some(i) = app.ui.sidebar_state.selected() {
                        select_room(app, i).await;
                    }
                }
                _ => {}
            }
        }
    }
}

pub(crate) async fn handle_help_key(app: &mut App, key: event::KeyEvent) {
    use crossterm::event::KeyCode;

    let max = super::commands::HELP_COMMANDS.len().saturating_sub(1);
    match key.code {
        KeyCode::Esc => {
            app.input.show_help = false;
        }
        KeyCode::Up => {
            let i = app.input.help_state.selected().unwrap_or(0);
            app.input.help_state.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down => {
            let i = app.input.help_state.selected().unwrap_or(0);
            app.input.help_state.select(Some((i + 1).min(max)));
        }
        KeyCode::Enter => {
            if let Some(i) = app.input.help_state.selected() {
                apply_help_selection(app, i);
            }
        }
        _ => {}
    }
}

pub(crate) async fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    if app.input.show_help {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(i) = help_row_at(app, mouse.column, mouse.row) {
                    apply_help_selection(app, i);
                } else if !rect_contains(app.input.help_area, mouse.column, mouse.row) {
                    app.input.show_help = false;
                }
            }
            MouseEventKind::ScrollUp => {
                let i = app.input.help_state.selected().unwrap_or(0);
                app.input.help_state.select(Some(i.saturating_sub(1)));
            }
            MouseEventKind::ScrollDown => {
                let max = super::commands::HELP_COMMANDS.len().saturating_sub(1);
                let i = app.input.help_state.selected().unwrap_or(0);
                app.input.help_state.select(Some((i + 1).min(max)));
            }
            _ => {}
        }
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if rect_contains(app.ui.sidebar_area, mouse.column, mouse.row) {
                app.input.focus = FocusPane::Sidebar;
                if let Some(i) = sidebar_row_at(app, mouse.column, mouse.row) {
                    select_room(app, i).await;
                }
            } else if rect_contains(app.ui.messages_area, mouse.column, mouse.row) {
                app.input.focus = FocusPane::Messages;
            } else if rect_contains(app.ui.input_area, mouse.column, mouse.row) {
                app.input.focus = FocusPane::Input;
            }
        }
        MouseEventKind::ScrollUp => {
            if rect_contains(app.ui.sidebar_area, mouse.column, mouse.row) {
                let i = app.ui.sidebar_state.selected().unwrap_or(0);
                app.ui.sidebar_state.select(Some(i.saturating_sub(1)));
            } else {
                let visible = app.ui.messages_area.height.saturating_sub(2) as usize;
                scroll_up(app, visible);
            }
        }
        MouseEventKind::ScrollDown => {
            if rect_contains(app.ui.sidebar_area, mouse.column, mouse.row) {
                let max = app.rooms.len().saturating_sub(1);
                let i = app.ui.sidebar_state.selected().unwrap_or(0);
                app.ui.sidebar_state.select(Some((i + 1).min(max)));
            } else {
                scroll_down(app);
            }
        }
        _ => {}
    }
}

pub(crate) fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
}

pub(crate) fn sidebar_row_at(app: &App, x: u16, y: u16) -> Option<usize> {
    let area = app.ui.sidebar_area;
    if
        x < area.x + 1 ||
        x >= area.x + area.width.saturating_sub(1) ||
        y < area.y + 1 ||
        y >= area.y + area.height.saturating_sub(1)
    {
        return None;
    }
    let row = ((y - (area.y + 1)) as usize) + app.ui.sidebar_state.offset();
    (row < app.rooms.len()).then_some(row)
}

pub(crate) fn help_row_at(app: &App, x: u16, y: u16) -> Option<usize> {
    let area = app.input.help_area;
    if
        x < area.x + 1 ||
        x >= area.x + area.width.saturating_sub(1) ||
        y < area.y + 1 ||
        y >= area.y + area.height.saturating_sub(1)
    {
        return None;
    }
    let row = ((y - (area.y + 1)) as usize) + app.input.help_state.offset();
    (row < super::commands::HELP_COMMANDS.len()).then_some(row)
}

pub(crate) async fn select_room(app: &mut App, i: usize) {
    if let Some(room) = app.rooms.get(i).cloned() {
        app.ui.sidebar_state.select(Some(i));
        if room != app.current_room {
            app.current_room = room.clone();
            app.scroll_offset = 0;
            super::server_msg::mark_all_read(app, &room).await;
            let wire = format!("/switch {}\n", room);
            let _ = app.writer.lock().await.write_all(wire.as_bytes()).await;
        }
    }
}

pub(crate) fn apply_help_selection(app: &mut App, i: usize) {
    if let Some((_, _, insert)) = super::commands::HELP_COMMANDS.get(i) {
        app.input.textarea.select_all();
        app.input.textarea.cut();
        app.input.textarea.insert_str(*insert);
    }
    app.input.show_help = false;
    app.input.focus = FocusPane::Input;
}

pub(crate) fn tab_forward(app: &mut App) {
    app.input.focus = match app.input.focus {
        FocusPane::Input => FocusPane::Messages,
        FocusPane::Messages => FocusPane::Sidebar,
        FocusPane::Sidebar => FocusPane::Input,
    };
}

pub(crate) fn tab_backward(app: &mut App) {
    app.input.focus = match app.input.focus {
        FocusPane::Input => FocusPane::Sidebar,
        FocusPane::Sidebar => FocusPane::Messages,
        FocusPane::Messages => FocusPane::Input,
    };
}

pub(crate) fn scroll_up(app: &mut App, visible_height: usize) {
    let content_width = app.ui.messages_area.width.saturating_sub(2) as usize;
    let total = super::server_msg::total_content_height(app, content_width as u16);
    let max = total.saturating_sub(visible_height as u16) as u16;
    app.scroll_offset = (app.scroll_offset + 1).min(max);
}

pub(crate) fn scroll_down(app: &mut App) {
    if app.scroll_offset > 0 {
        app.scroll_offset -= 1;
    }
}
