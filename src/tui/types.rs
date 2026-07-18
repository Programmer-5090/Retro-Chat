use std::collections::HashSet;

use ratatui::style::Color;

use crate::ChatMessage;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub bg: Color,
    pub success: Color,
}

pub const THEMES: &[Theme] = &[
    Theme {
        name: "Amber",
        primary: Color::Rgb(255, 176, 0),
        secondary: Color::Rgb(255, 120, 0),
        accent: Color::Cyan,
        bg: Color::Black,
        success: Color::Rgb(80, 220, 130),
    },
    Theme {
        name: "Matrix",
        primary: Color::Rgb(0, 200, 0),
        secondary: Color::Rgb(0, 160, 0),
        accent: Color::Rgb(0, 255, 65),
        bg: Color::Black,
        success: Color::Rgb(100, 255, 100),
    },
    Theme {
        name: "Synthwave",
        primary: Color::Rgb(255, 50, 150),
        secondary: Color::Rgb(200, 50, 120),
        accent: Color::Rgb(0, 200, 255),
        bg: Color::Rgb(10, 0, 20),
        success: Color::Rgb(100, 255, 200),
    },
    Theme {
        name: "Solarized",
        primary: Color::Rgb(181, 137, 0),
        secondary: Color::Rgb(203, 75, 22),
        accent: Color::Rgb(42, 161, 152),
        bg: Color::Rgb(0, 43, 54),
        success: Color::Rgb(133, 153, 0),
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Input,
    Messages,
    Sidebar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationKind {
    Cube,
    MatrixRain,
    Starfield,
    Torus,
    Sand,
}

impl AnimationKind {
    pub fn next(self) -> Self {
        match self {
            AnimationKind::Cube => AnimationKind::MatrixRain,
            AnimationKind::MatrixRain => AnimationKind::Starfield,
            AnimationKind::Starfield => AnimationKind::Torus,
            AnimationKind::Torus => AnimationKind::Sand,
            AnimationKind::Sand => AnimationKind::Cube,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AnimationKind::Cube => "Cube",
            AnimationKind::MatrixRain => "Matrix",
            AnimationKind::Starfield => "Warp",
            AnimationKind::Torus => "Torus",
            AnimationKind::Sand => "Sand",
        }
    }
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
    pub messages: Vec<ChatMessage>,
    pub focus: FocusPane,
    pub should_quit: bool,
    pub unread_rooms: HashSet<String>,
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
            unread_rooms: HashSet::new(),
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

    fn room_message_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.room == self.current_room || m.room.is_empty())
            .count()
    }

    pub fn scroll_up(&mut self, visible_height: usize) {
        let count = self.room_message_count();
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = (self.scroll_offset + 1).min(max);
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    pub fn clear_messages(&mut self) {
        self.messages.retain(|m| m.room != self.current_room && !m.room.is_empty());
        self.scroll_offset = 0;
    }

    pub fn clamp_scroll(&mut self, visible_height: usize) {
        let count = self.room_message_count();
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = self.scroll_offset.min(max);
    }

    pub fn append_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }
}
