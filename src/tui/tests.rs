use proptest::prelude::*;

use super::types::{FocusPane, THEMES, UiAppState};
use ratatui::style::Color;
use super::render::{border_style, format_gradient_title, format_user_message, format_system_message};

use crate::ChatMessage;
use crate::message::MessageType;

fn arb_message_type() -> impl Strategy<Value = MessageType> {
    prop_oneof![
        Just(MessageType::UserMessage),
        Just(MessageType::SystemNotification),
    ]
}

prop_compose! {
    fn arb_chat_message()(
        username in "[a-zA-Z][a-zA-Z0-9_]{0,63}",
        content in ".{0,200}",
        timestamp in "[0-9]{2}:[0-9]{2}:[0-9]{2}",
        message_type in arb_message_type(),
    ) -> ChatMessage {
        ChatMessage { id: String::new(), username, content, timestamp, message_type, room: String::new(), is_history: false }
    }
}

fn arb_focus_pane() -> impl Strategy<Value = FocusPane> {
    prop_oneof![
        Just(FocusPane::Input),
        Just(FocusPane::Messages),
        Just(FocusPane::Sidebar),
    ]
}

fn arb_non_empty_room_name() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,16}".prop_filter("non-empty", |s| !s.is_empty())
}

// Scroll offset is always bounded
proptest! {
    #[test]
    fn prop_scroll_bounded(
        initial_offset in 0u16..100u16,
        msg_count in 0usize..100usize,
        visible_height in 1usize..30usize,
        ops in prop::collection::vec(prop_oneof![Just("up"), Just("down")], 0usize..20usize),
    ) {
        let mut state = UiAppState::new();
        for _ in 0..msg_count {
            state.messages.push(ChatMessage {
                id: String::new(),
                username: "u".into(),
                content: "c".into(),
                timestamp: "00:00:00".into(),
                message_type: MessageType::UserMessage,
                room: "general".into(),
                is_history: false,
            });
        }
        state.scroll_offset = initial_offset.min(state.messages.len().saturating_sub(visible_height) as u16);

        for op in &ops {
            match *op {
                "up" => state.scroll_up(visible_height),
                "down" => state.scroll_down(),
                _ => {}
            }
            let max = state.messages.len().saturating_sub(visible_height) as u16;
            assert!(state.scroll_offset <= max, "scroll_offset={} > max={}", state.scroll_offset, max);
        }
    }
}

// rooms list never becomes empty
proptest! {
    #[test]
    fn prop_rooms_never_empty(
        room_names in prop::collection::vec(arb_non_empty_room_name(), 1usize..10usize),
        ops in prop::collection::vec(prop_oneof![Just("join"), Just("join"), Just("leave")], 0usize..30usize),
    ) {
        let mut state = UiAppState::new();
        let mut name_idx = 0usize;

        for op in &ops {
            match *op {
                "join" => {
                    let room = &room_names[name_idx % room_names.len()];
                    name_idx += 1;
                    state.join_room(room);
                }
                "leave" => {
                    let _ = state.leave_room();
                }
                _ => {}
            }
            assert!(!state.rooms.is_empty(), "rooms became empty");
        }
    }
}

// current_room is always a member of rooms
proptest! {
    #[test]
    fn prop_current_room_in_rooms(
        room_names in prop::collection::vec(arb_non_empty_room_name(), 2usize..8usize),
        ops in prop::collection::vec(prop_oneof![Just("join"), Just("join"), Just("join"), Just("leave")], 0usize..30usize),
    ) {
        let mut state = UiAppState::new();
        let mut name_idx = 0usize;

        for op in &ops {
            match *op {
                "join" => {
                    let room = &room_names[name_idx % room_names.len()];
                    name_idx += 1;
                    state.join_room(room);
                }
                "leave" => {
                    let _ = state.leave_room();
                }
                _ => {}
            }
            assert!(state.rooms.contains(&state.current_room),
                "current_room={} not in rooms={:?}", state.current_room, state.rooms);
        }
    }
}

// Tab forward cycles through all focus states
proptest! {
    #[test]
    fn prop_tab_forward_cycles(focus in arb_focus_pane()) {
        let mut state = UiAppState::new();
        state.focus = focus;
        state.tab_forward();
        state.tab_forward();
        state.tab_forward();
        assert_eq!(state.focus, focus, "tab_forward did not cycle back");
    }
}

// Tab backward cycles through all focus states
proptest! {
    #[test]
    fn prop_tab_backward_cycles(focus in arb_focus_pane()) {
        let mut state = UiAppState::new();
        state.focus = focus;
        state.tab_backward();
        state.tab_backward();
        state.tab_backward();
        assert_eq!(state.focus, focus, "tab_backward did not cycle back");
    }
}

// border_style returns correct colours
proptest! {
    #[test]
    fn prop_border_style_colours(pane in arb_focus_pane(), focus in arb_focus_pane()) {
        let style = border_style(pane, focus, 0, &THEMES[0]);
        if pane == focus {
            assert!(style.fg.is_some(), "focused pane should have a border color");
            assert_ne!(style.fg.unwrap(), THEMES[0].primary, "focused pane should not be PRIMARY");
        } else {
            assert_eq!(style.fg.unwrap(), THEMES[0].primary, "unfocused pane should have AMBER border");
        }
    }
}

// Title bar contains username and app name
proptest! {
    #[test]
    fn prop_title_contains_username(username in "[a-zA-Z0-9_]{1,32}") {
        let line = format_gradient_title(&username);
        let title: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(title.contains("RETRO CHAT"), "title missing app name: {title}");
        assert!(title.contains(&format!("@{}", username)), "title missing username: {title}");
    }
}

#[test]
fn test_mention_highlighting() {
    let msg = ChatMessage {
        id: String::new(),
        username: "alice".into(),
        content: "hi @bob how are you".into(),
        timestamp: "12:34:56".into(),
        message_type: MessageType::UserMessage,
        room: String::new(),
        is_history: false,
    };
    let lines = format_user_message(&msg, Color::Rgb(255, 176, 0), Color::Cyan, None);
    let rendered: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(rendered.contains("@bob"), "rendered should contain @bob");
    assert!(rendered.contains("hi "), "rendered should contain 'hi '");
    // Check that @bob span is actually styled cyan
    assert!(lines.len() == 1, "should have 1 line");
    let spans = &lines[0].spans;
    let mention_spans: Vec<_> = spans.iter().filter(|s| s.content == "@bob").collect();
    assert!(!mention_spans.is_empty(), "should have a span with @bob content");
    for s in &mention_spans {
        assert_eq!(
            s.style.fg,
            Some(Color::Cyan),
            "@bob span should be cyan, got {:?}",
            s.style.fg
        );
        assert!(
            s.style.add_modifier.contains(ratatui::style::Modifier::BOLD),
            "@bob span should be bold"
        );
    }
}

// UserMessage format contains required fields
proptest! {
    #[test]
    fn prop_user_message_format(msg in arb_chat_message().prop_filter("must be UserMessage", |m| matches!(m.message_type, MessageType::UserMessage))) {
        let lines = format_user_message(&msg, Color::Rgb(255, 176, 0), Color::Cyan, None);
        let rendered: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(rendered.contains(&msg.username), "missing username");
        assert!(rendered.contains("\u{25B6}"), "missing ▶ separator");
        assert!(rendered.contains(&msg.timestamp[..5]), "missing timestamp prefix");
        assert!(rendered.contains(&msg.content), "missing content");
    }
}

// SystemNotification format matches *** pattern
proptest! {
    #[test]
    fn prop_system_message_format(msg in arb_chat_message().prop_filter("must be SystemNotification", |m| matches!(m.message_type, MessageType::SystemNotification))) {
        let lines = format_system_message(&msg, Color::Cyan);
        let rendered: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(rendered.starts_with("*** "), "should start with ***");
        assert!(rendered.ends_with(" ***"), "should end with ***");
        assert!(rendered.contains(&msg.content), "missing content");
    }
}