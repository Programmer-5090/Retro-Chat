use ratatui::style::Color;

use crate::ChatMessage;

pub const AMBER: Color = Color::Rgb(255, 176, 0);
pub const CYAN: Color = Color::Cyan;
pub const BLACK: Color = Color::Black;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Input,
    Messages,
    Sidebar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum LeaveResult {
    Left,
    CannotLeaveLast,
}

#[allow(dead_code)]
pub struct UiAppState {
    pub rooms: Vec<String>,
    pub current_room: String,
    pub scroll_offset: u16,
    pub messages: Vec<(String, ChatMessage)>,
    pub focus: FocusPane,
    pub should_quit: bool,
}

#[allow(dead_code)]
impl UiAppState {
    pub fn new() -> Self {
        Self {
            rooms: vec!["general".to_string()],
            current_room: "general".to_string(),
            scroll_offset: 0,
            focus: FocusPane::Input,
            messages: vec![],
            should_quit: false,
        }
    }

    pub fn join_room(&mut self, room: &str) {
        if !self.rooms.iter().any(|r| r == room) {
            self.rooms.push(room.to_string());
        }
        self.current_room = room.to_string();
    }

    pub fn leave_room(&mut self) -> LeaveResult {
        if self.rooms.len() > 1 {
            self.rooms.retain(|r| r != &self.current_room);
            self.current_room = self.rooms[0].clone();
            LeaveResult::Left
        } else {
            LeaveResult::CannotLeaveLast
        }
    }

    pub fn tab_forward(&mut self) {
        self.focus = match self.focus {
            FocusPane::Input => FocusPane::Messages,
            FocusPane::Messages => FocusPane::Sidebar,
            FocusPane::Sidebar => FocusPane::Input,
        };
    }

    pub fn tab_backward(&mut self) {
        self.focus = match self.focus {
            FocusPane::Input => FocusPane::Sidebar,
            FocusPane::Sidebar => FocusPane::Messages,
            FocusPane::Messages => FocusPane::Input,
        };
    }

    fn current_room_msg_count(&self) -> usize {
        self.messages.iter().filter(|(r, _)| r == &self.current_room).count()
    }

    pub fn scroll_up(&mut self, visible_height: usize) {
        let count = self.current_room_msg_count();
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = (self.scroll_offset + 1).min(max);
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    pub fn clear_messages(&mut self) {
        self.messages.retain(|(r, _)| r != &self.current_room);
        self.scroll_offset = 0;
    }

    pub fn clamp_scroll(&mut self, visible_height: usize) {
        let count = self.current_room_msg_count();
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = self.scroll_offset.min(max);
    }

    pub fn append_message(&mut self, msg: ChatMessage) {
        self.messages.push((self.current_room.clone(), msg));
    }
}
