use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    prelude::{CrosstermBackend, Terminal},
    style::{Color, Style},
    symbols::border,
    widgets::{Block, Borders, Paragraph},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{Mutex, mpsc},
};
use tui_textarea::TextArea;

use crate::client_helpers::ClientStream;

use super::render::{border_style, format_title, format_user_message, format_system_message, make_system_msg};
use super::types::{AMBER, BLACK, CYAN, FocusPane};

use crate::ChatMessage;
use crate::message::MessageType;

pub async fn run_chat_ui(
    username: String,
    reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
    writer: tokio::io::WriteHalf<ClientStream>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (server_tx, server_rx) = mpsc::unbounded_channel::<String>();
    let mut app = App::new(username, writer, server_rx);
    app.run(reader, server_tx).await
}

pub(crate) struct App {
    pub(crate) username: String,
    pub(crate) rooms: Vec<String>,
    pub(crate) current_room: String,
    messages: Vec<(String, ChatMessage)>,
    pub(crate) scroll_offset: u16,
    pub(crate) focus: FocusPane,
    pub(crate) textarea: TextArea<'static>,
    pub(crate) writer: Arc<Mutex<tokio::io::WriteHalf<ClientStream>>>,
    pub(crate) should_quit: bool,
    pub(crate) server_rx: mpsc::UnboundedReceiver<String>,
    sidebar_scroll: usize,
}

impl App {
    fn new(
        username: String,
        writer: tokio::io::WriteHalf<ClientStream>,
        server_rx: mpsc::UnboundedReceiver<String>,
    ) -> Self {
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        ta.set_style(Style::default().fg(AMBER).bg(BLACK));
        ta.set_cursor_style(Style::default().bg(AMBER));

        Self {
            username,
            rooms: vec!["general".to_string()],
            current_room: "general".to_string(),
            focus: FocusPane::Input,
            messages: Vec::new(),
            scroll_offset: 0,
            should_quit: false,
            textarea: ta,
            writer: Arc::new(Mutex::new(writer)),
            server_rx,
            sidebar_scroll: 0,
        }
    }

    async fn run(
        &mut self,
        reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
        server_tx: mpsc::UnboundedSender<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

        tokio::spawn(async move {
            let mut lines = reader.lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if server_tx.send(line).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = server_tx.send("__CONN_CLOSED__".to_string());
                        break;
                    }
                    Err(_) => break,
                }
            }
        });

        let res = self.event_loop(&mut terminal).await;

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        terminal.show_cursor()?;

        let _ = self.writer.lock().await.shutdown().await;
        res
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|f| self.render(f))?;
            if event::poll(Duration::from_millis(16))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key).await;
                    }
                }
            }
            loop {
                match self.server_rx.try_recv() {
                    Ok(line) => self.handle_server_message(&line),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        self.push_msg(make_system_msg("Internal channel error"));
                        self.should_quit = true;
                        break;
                    }
                }
            }
            if self.should_quit {
                break;
            }
        }
        Ok(())
    }

    async fn handle_key(&mut self, key: event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        match key.code {
            KeyCode::Esc => {
                self.should_quit = true;
                return;
            }
            KeyCode::Tab => {
                self.tab_forward();
                return;
            }
            KeyCode::BackTab => {
                self.tab_backward();
                return;
            }
            KeyCode::F(1) => {
                self.focus = FocusPane::Sidebar;
                return;
            }
            _ => {}
        }

        match self.focus {
            FocusPane::Input => {
                match key.code {
                    KeyCode::Enter => {
                        let text = self.textarea.lines().first().cloned().unwrap_or_default();
                        self.textarea.select_all();
                        self.textarea.cut();
                        self.send_or_command(text).await;
                    }
                    KeyCode::Char(_) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let current_len = self.textarea
                            .lines()
                            .first()
                            .map(|l| l.len())
                            .unwrap_or(0);
                        if current_len < 500 {
                            self.textarea.input(key);
                        }
                    }
                    | KeyCode::Backspace
                    | KeyCode::Delete
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End => {
                        self.textarea.input(key);
                    }
                    _ => {}
                }
            }
            FocusPane::Messages => {
                match key.code {
                    KeyCode::Up => {
                        let visible = /* approximate */ 20;
                        self.scroll_up(visible);
                    }
                    KeyCode::Down => {
                        self.scroll_down();
                    }
                    _ => {}
                }
            }
            FocusPane::Sidebar => {
                let max = self.rooms.len().saturating_sub(1);
                match key.code {
                    KeyCode::Up if self.sidebar_scroll > 0 => {
                        self.sidebar_scroll -= 1;
                    }
                    KeyCode::Down if self.sidebar_scroll < max => {
                        self.sidebar_scroll += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    fn push_msg(&mut self, msg: ChatMessage) {
        self.messages.push((self.current_room.clone(), msg));
    }

    fn current_room_msgs(&self) -> impl Iterator<Item = &ChatMessage> {
        self.messages.iter().filter_map(move |(r, m)| {
            if r == &self.current_room { Some(m) } else { None }
        })
    }

    fn current_room_msg_count(&self) -> usize {
        self.messages.iter().filter(|(r, _)| r == &self.current_room).count()
    }

    async fn send_or_command(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }

        if text.starts_with('/') {
            let parts: Vec<&str> = text.splitn(3, ' ').collect();
            let cmd = parts[0];

            match cmd {
                "/help" => {
                    let help_text = "Commands:\n\
                        /join <room>    — join a room\n\
                        /leave          — leave current room\n\
                        /rooms          — list server rooms\n\
                        /dm <user> <msg> — direct message\n\
                        /clear          — clear messages\n\
                        /help           — show this help\n\
                        /quit           — quit";
                    self.push_msg(make_system_msg(help_text));
                }
                "/clear" => {
                    self.messages.retain(|(r, _)| r != &self.current_room);
                    self.scroll_offset = 0;
                }
                "/quit" => {
                    self.should_quit = true;
                    let _ = self.writer.lock().await.shutdown().await;
                }
                "/join" => {
                    let room = parts.get(1).copied().unwrap_or("").trim();
                    if room.is_empty() || room.len() > 32 || room.contains(char::is_whitespace) {
                        self.push_msg(
                            make_system_msg("Usage: /join <room>  (1–32 non-whitespace chars)"),
                        );
                    } else {
                        if !self.rooms.iter().any(|r| r == room) {
                            self.rooms.push(room.to_string());
                        }
                        self.messages.retain(|(r, _)| r != &self.current_room);
                        self.current_room = room.to_string();
                        self.scroll_offset = 0;
                        self.push_msg(make_system_msg(&format!("Joined room: {}", room)));
                        let msg = format!("/join {}\n", room);
                        let _ = self.writer.lock().await.write_all(msg.as_bytes()).await;
                    }
                }
                "/leave" => {
                    if self.rooms.len() > 1 {
                        let left = self.current_room.clone();
                        self.rooms.retain(|r| r != &self.current_room);
                        self.current_room = self.rooms[0].clone();
                        self.push_msg(make_system_msg(&format!("Left room: {}", left)));
                        let _ = self.writer.lock().await.write_all(b"/leave\n").await;
                    } else {
                        self.push_msg(make_system_msg("Cannot leave the last room."));
                    }
                }
                "/rooms" => {
                    let _ = self.writer.lock().await.write_all(b"/rooms\n").await;
                }
                "/dm" => {
                    let user = parts.get(1).copied().unwrap_or("").trim();
                    let dm_msg = parts.get(2).copied().unwrap_or("").trim();
                    if user.is_empty() || dm_msg.is_empty() {
                        self.push_msg(make_system_msg("Usage: /dm <user> <message>"));
                    } else {
                        let mut users = vec![self.username.clone(), user.to_string()];
                        users.sort();
                        let dm_room = format!("__dm__{}", users.join("_"));
                        if !self.rooms.iter().any(|r| r == &dm_room) {
                            self.rooms.push(dm_room.clone());
                        }
                        self.messages.retain(|(r, _)| r != &self.current_room);
                        self.current_room = dm_room;
                        self.scroll_offset = 0;
                        let wire = format!("/msg {} {}\n", user, dm_msg);
                        let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
                    }
                }
                _ => {
                    self.push_msg(make_system_msg(&format!("Unknown command: {}", text)));
                }
            }
        } else {
            let wire = format!("{}\n", text);
            let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
        }
    }

    fn handle_server_message(&mut self, line: &str) {
        if line == "__CONN_CLOSED__" {
            self.push_msg(make_system_msg("Connection closed by server"));
            self.should_quit = true;
            return;
        }

        if let Ok(msg) = serde_json::from_str::<ChatMessage>(line) {
            match msg.message_type {
                MessageType::RoomList => {
                    self.rooms = msg.content
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if self.rooms.is_empty() {
                        self.rooms.push("general".to_string());
                    }
                    if !self.rooms.iter().any(|r| r == &self.current_room) {
                        self.current_room = self.rooms[0].clone();
                    }
                }
                _ => {
                    self.push_msg(msg);
                    let visible: usize = 20;
                    self.clamp_scroll(visible);
                }
            }
        }
    }

    fn tab_forward(&mut self) {
        self.focus = match self.focus {
            FocusPane::Input => FocusPane::Messages,
            FocusPane::Messages => FocusPane::Sidebar,
            FocusPane::Sidebar => FocusPane::Input,
        };
    }

    fn tab_backward(&mut self) {
        self.focus = match self.focus {
            FocusPane::Input => FocusPane::Sidebar,
            FocusPane::Sidebar => FocusPane::Messages,
            FocusPane::Messages => FocusPane::Input,
        };
    }

    fn scroll_up(&mut self, visible_height: usize) {
        let count = self.current_room_msg_count();
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = (self.scroll_offset + 1).min(max);
    }

    fn scroll_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    fn clamp_scroll(&mut self, visible_height: usize) {
        let count = self.current_room_msg_count();
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = self.scroll_offset.min(max);
    }

    fn render_title_bar(&self, f: &mut Frame, area: Rect) {
        let title = format_title(&self.username);
        let widget = Paragraph::new(title)
            .style(Style::default().fg(AMBER))
            .alignment(Alignment::Center);
        f.render_widget(widget, area);
    }

    fn render_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let lines: Vec<ratatui::text::Line> = self.rooms
            .iter()
            .map(|room| {
                let style = if room == &self.current_room {
                    Style::default().fg(CYAN)
                } else {
                    Style::default().fg(AMBER)
                };
                ratatui::text::Line::from(ratatui::text::Span::styled(room.as_str(), style))
            })
            .collect();

        let inner_h = area.height.saturating_sub(2) as usize;
        let max_scroll = self.rooms.len().saturating_sub(inner_h);
        self.sidebar_scroll = self.sidebar_scroll.min(max_scroll);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(border_style(FocusPane::Sidebar, self.focus))
            .title(" Rooms ");

        let paragraph = Paragraph::new(lines)
            .block(block)
            .scroll((self.sidebar_scroll as u16, 0));
        f.render_widget(paragraph, area);
    }

    fn render_messages(&self, f: &mut Frame, area: Rect) {
        let lines: Vec<ratatui::text::Line> = self.current_room_msgs()
            .map(|msg| match msg.message_type {
                MessageType::UserMessage => format_user_message(msg),
                MessageType::SystemNotification => format_system_message(msg),
                MessageType::RoomList => unreachable!(),
            })
            .collect();

        let total_lines = lines.len() as u16;
        let visible_height = area.height.saturating_sub(2);
        let max_scroll = total_lines.saturating_sub(visible_height);
        let render_scroll = if self.scroll_offset == 0 {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(border_style(FocusPane::Messages, self.focus))
            .title(" Messages ");

        let paragraph = Paragraph::new(lines).block(block).scroll((render_scroll, 0));
        f.render_widget(paragraph, area);
    }

    fn render_input(&mut self, f: &mut Frame, area: Rect) {
        if self.focus == FocusPane::Input {
            self.textarea.set_cursor_style(Style::default().bg(CYAN));
        } else {
            self.textarea.set_cursor_style(Style::default());
        }
        self.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(border::ROUNDED)
                .border_style(border_style(FocusPane::Input, self.focus))
                .title(" Message "),
        );
        f.render_widget(&self.textarea, area);
    }

    fn render_status_bar(&self, f: &mut Frame, area: Rect) {
        let status = Paragraph::new("F1:rooms  \u{2191}\u{2193}:scroll  Tab:focus")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(status, area);
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();

        if area.width < 17 || area.height < 6 {
            let msg = Paragraph::new("Terminal too small \u{2014} please resize")
                .style(Style::default().fg(AMBER).bg(BLACK));
            f.render_widget(msg, area);
            return;
        }

        let vertical = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(1),
        ]);
        let [title_area, body_area, input_area, status_area] = vertical.areas(area);

        let horizontal = Layout::horizontal([
            Constraint::Length(16),
            Constraint::Min(0),
        ]);
        let [sidebar_area, messages_area] = horizontal.areas(body_area);

        self.render_title_bar(f, title_area);
        self.render_sidebar(f, sidebar_area);
        self.render_messages(f, messages_area);
        self.render_input(f, input_area);
        self.render_status_bar(f, status_area);
    }
}
