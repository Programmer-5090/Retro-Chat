use std::{ collections::{ HashMap, HashSet, VecDeque }, sync::Arc, time::{ Duration, Instant } };

use crossterm::{
    event::{
        self,
        DisableBracketedPaste,
        DisableMouseCapture,
        EnableBracketedPaste,
        EnableMouseCapture,
        Event,
        KeyEventKind,
        MouseButton,
        MouseEvent,
        MouseEventKind,
    },
    execute,
    terminal::{ EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode },
};
use ratatui::{
    Frame,
    layout::{ Constraint, Layout, Rect },
    prelude::{ CrosstermBackend, Terminal },
    style::{ Color, Modifier, Style },
    symbols::border,
    widgets::{ Block, Borders, Clear, List, ListItem, ListState, Paragraph, Sparkline, Wrap },
};
use tokio::{ io::{ AsyncBufReadExt, AsyncWriteExt, BufReader }, sync::{ Mutex, mpsc } };
use tui_textarea::TextArea;

use crate::client_helpers::ClientStream;
use crate::message::MessageType;
use crate::message::dm_display_name;
use crate::ChatMessage;

use super::render::{
    border_style,
    format_title,
    format_system_message,
    format_user_message,
    make_system_msg,
    username_color,
};
use super::anims::{ SpinningCube, MatrixRain, Starfield, SpinningTorus, SandSim };
use super::types::{ AnimationKind, FocusPane, Theme, THEMES };

pub async fn run_chat_ui(
    username: String,
    reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
    writer: tokio::io::WriteHalf<ClientStream>
) -> Result<(), Box<dyn std::error::Error>> {
    let (server_tx, server_rx) = mpsc::unbounded_channel::<String>();
    let mut app = App::new(username, writer, server_rx);
    app.run(reader, server_tx).await
}

struct CleanGuard;

impl Drop for CleanGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste);
    }
}

pub struct App {
    pub username: String,
    pub rooms: Vec<String>,
    pub current_room: String,
    messages: Vec<(ChatMessage, bool)>,
    pub scroll_offset: u16,
    pub focus: FocusPane,
    pub textarea: TextArea<'static>,
    pub writer: Arc<Mutex<tokio::io::WriteHalf<ClientStream>>>,
    pub should_quit: bool,
    pub server_rx: mpsc::UnboundedReceiver<String>,
    sidebar_state: ListState,
    sidebar_area: Rect,
    messages_area: Rect,
    input_area: Rect,
    show_help: bool,
    help_state: ListState,
    help_area: Rect,
    unread_rooms: HashSet<String>,
    /// Ids of your own messages that the recipient has read (per the
    /// ReadReceipt broadcasts), rendered in the "read" color.
    read_message_ids: HashSet<String>,
    message_times: VecDeque<Instant>,
    sparkline_data: VecDeque<u16>,
    pulse_tick: u64,
    theme: Theme,
    theme_idx: usize,
    online_users: HashSet<String>,
    typing_users: HashMap<String, (String, Instant)>,
    last_keypress: Instant,
    last_typing_sent: Instant,
    cube: SpinningCube,
    matrix_rain: MatrixRain,
    starfield: Starfield,
    torus: SpinningTorus,
    sand: SandSim,
    anim_kind: AnimationKind,
    start_time: Instant,
}

impl App {
    /// (display command, description, text inserted into the input box
    /// when picked from the `/help` popup). Argument-taking commands insert
    /// a trailing space so the cursor lands ready to type the argument.
    const HELP_COMMANDS: [(&'static str, &'static str, &'static str); 7] = [
        ("/join <room>", "join a room", "/join "),
        ("/leave", "leave current room", "/leave"),
        ("/rooms", "list server rooms", "/rooms"),
        ("/dm <user> <msg>", "send a direct message", "/dm "),
        ("/clear", "clear messages", "/clear"),
        ("/help", "show this help", "/help"),
        ("/quit", "quit", "/quit"),
    ];

    fn new(
        username: String,
        writer: tokio::io::WriteHalf<ClientStream>,
        server_rx: mpsc::UnboundedReceiver<String>
    ) -> Self {
        let default_theme = &THEMES[0];
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        ta.set_style(Style::default().fg(default_theme.primary).bg(default_theme.bg));
        ta.set_cursor_style(Style::default().bg(default_theme.primary));

        let mut app = Self {
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
            sidebar_state: ListState::default().with_selected(Some(0)),
            sidebar_area: Rect::default(),
            messages_area: Rect::default(),
            input_area: Rect::default(),
            show_help: false,
            help_state: ListState::default().with_selected(Some(0)),
            help_area: Rect::default(),
            unread_rooms: HashSet::new(),
            read_message_ids: HashSet::new(),
            message_times: VecDeque::new(),
            sparkline_data: VecDeque::from(vec![0u16; 40]),
            pulse_tick: 0,
            theme: *default_theme,
            theme_idx: 0,
            online_users: HashSet::new(),
            typing_users: HashMap::new(),
            last_keypress: Instant::now(),
            last_typing_sent: Instant::now(),
            cube: SpinningCube::new(),
            matrix_rain: MatrixRain::new(),
            starfield: Starfield::new(),
            torus: SpinningTorus::new(),
            sand: SandSim::new(),
            anim_kind: AnimationKind::Cube,
            start_time: Instant::now(),
        };
        app.cube.color = default_theme.primary;
        app.matrix_rain.color = default_theme.primary;
        app.starfield.color = default_theme.primary;
        app.torus.color = default_theme.primary;
        app.sand.color = default_theme.primary;
        app
    }

    async fn run(
        &mut self,
        reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
        server_tx: mpsc::UnboundedSender<String>
    ) -> Result<(), Box<dyn std::error::Error>> {
        enable_raw_mode()?;
        let _guard = CleanGuard;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
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
                    Err(_) => {
                        break;
                    }
                }
            }
        });

        let res = self.event_loop(&mut terminal).await;
        let _ = self.writer.lock().await.shutdown().await;
        res
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut last_sparkline_tick = Instant::now();
        loop {
            terminal.draw(|f| self.render(f))?;

            self.pulse_tick = self.pulse_tick.wrapping_add(1);
            let now = Instant::now();
            if now - last_sparkline_tick >= Duration::from_secs(1) {
                let cutoff = now - Duration::from_secs(2);
                let count = self.message_times
                    .iter()
                    .filter(|t| **t > cutoff)
                    .count() as u16;
                self.sparkline_data.push_back(count);
                self.sparkline_data.pop_front();
                last_sparkline_tick = now;
            }

            // Typing indicator debounce
            let now = Instant::now();
            let since_keypress = now - self.last_keypress;
            let since_typing_sent = now - self.last_typing_sent;
            let input_text = self.textarea.lines().first().cloned().unwrap_or_default();
            if
                self.focus == FocusPane::Input &&
                since_keypress < Duration::from_secs(2) &&
                since_typing_sent > Duration::from_secs(3)
            {
                let wire = if input_text.starts_with('/') {
                    // Only send typing for /dm composition, not other commands
                    input_text
                        .strip_prefix("/dm ")
                        .and_then(|s| s.split_whitespace().next())
                        .filter(|t| !t.is_empty())
                        .map(|target| {
                            let mut users = vec![self.username.clone(), target.to_string()];
                            users.sort();
                            format!("/typing __dm__{}\n", users.join("_"))
                        })
                        .unwrap_or_default()
                } else {
                    format!("/typing {}\n", self.current_room)
                };
                if !wire.is_empty() {
                    let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
                    self.last_typing_sent = now;
                }
            }
            // Clean stale typing indicators
            self.typing_users.retain(|_, (_, t)| now - *t < Duration::from_secs(4));

            if event::poll(Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            self.handle_key(key).await;
                        }
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse(mouse).await;
                    }
                    Event::Paste(data) => {
                        if self.focus == FocusPane::Input {
                            let text = data.lines().next().unwrap_or(&data);
                            let current_len = self.textarea
                                .lines()
                                .first()
                                .map(|l| l.len())
                                .unwrap_or(0);
                            let paste_len = text.len();
                            if current_len + paste_len <= 500 {
                                self.textarea.insert_str(text);
                                self.last_keypress = Instant::now();
                            }
                        }
                    }
                    _ => {}
                }
            }
            loop {
                match self.server_rx.try_recv() {
                    Ok(line) => self.handle_server_message(&line).await,
                    Err(mpsc::error::TryRecvError::Empty) => {
                        break;
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        self.ingest_msg(make_system_msg("Internal channel error"), true);
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

    fn ingest_msg(&mut self, msg: ChatMessage, read: bool) {
        self.message_times.push_back(Instant::now());
        if self.message_times.len() > 200 {
            self.message_times.pop_front();
        }

        let was_unread = !read && !msg.room.is_empty() && msg.room != self.current_room;
        if was_unread {
            self.unread_rooms.insert(msg.room.clone());
        }

        self.messages.push((msg, was_unread));
        let visible: usize = 20;
        self.clamp_scroll(visible);
    }

    /// Locally resets any lingering "unread" color for messages in `room`
    /// back to normal, without notifying anyone. Used when a live message
    /// arrives in the room you're currently viewing — per spec, the room
    /// being actively watched means older unread messages should no longer
    /// look unread. This is intentionally NOT called just from switching
    /// rooms (see `handle_server_message`'s `is_history` check), so that
    /// leaving and re-entering a room without any new activity still shows
    /// the same unread coloring you left behind.
    fn clear_room_read_state(&mut self, room: &str) {
        for pair in &mut self.messages {
            let same_room =
                pair.0.room == room || (pair.0.room.is_empty() && room == self.current_room);
            if same_room {
                pair.1 = false;
            }
        }
        self.unread_rooms.remove(room);
    }

    /// Ids of currently-unread messages authored by someone else in `room`,
    /// used to build a read-receipt payload.
    fn collect_unread_ids(&self, room: &str) -> Vec<String> {
        self.messages
            .iter()
            .filter(|(msg, was_unread)| {
                *was_unread &&
                    msg.username != self.username &&
                    !msg.id.is_empty() &&
                    (msg.room == room || (msg.room.is_empty() && room == self.current_room))
            })
            .map(|(msg, _)| msg.id.clone())
            .collect()
    }

    /// Marks everything in `room` as read locally AND tells the server, so
    /// the original senders can see their messages flip to the "read"
    /// color. Only call this for a deliberate read action (you replying in
    /// the room) — calling it on every incoming message would flood the
    /// server with acks during history replay.
    async fn mark_all_read(&mut self, room: &str) {
        let ids = self.collect_unread_ids(room);
        self.clear_room_read_state(room);
        self.send_read_receipt(room, ids).await;
    }

    async fn send_read_receipt(&mut self, room: &str, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let wire = format!("/read {} {}\n", room, ids.join(","));
        let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
    }

    fn messages_for_room(&self, room: &str) -> impl Iterator<Item = &(ChatMessage, bool)> {
        self.messages
            .iter()
            .filter(move |(msg, _)| {
                msg.room == room || (msg.room.is_empty() && room == self.current_room)
            })
    }

    fn room_message_count(&self, room: &str) -> usize {
        self.messages
            .iter()
            .filter(
                |(msg, _)| (msg.room == room || (msg.room.is_empty() && room == self.current_room))
            )
            .count()
    }

    async fn handle_key(&mut self, key: event::KeyEvent) {
        use crossterm::event::{ KeyCode, KeyModifiers };

        if self.show_help {
            self.handle_help_key(key).await;
            return;
        }

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
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.theme_idx = (self.theme_idx + 1) % THEMES.len();
                self.theme = THEMES[self.theme_idx];
                self.cube.color = self.theme.secondary;
                self.matrix_rain.color = self.theme.secondary;
                self.starfield.color = self.theme.secondary;
                self.torus.color = self.theme.secondary;
                self.sand.color = self.theme.secondary;
                self.textarea.set_style(Style::default().fg(self.theme.primary).bg(self.theme.bg));
                self.textarea.set_cursor_style(Style::default().bg(self.theme.primary));
                return;
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.anim_kind = self.anim_kind.next();
                return;
            }
            _ => {}
        }

        match self.focus {
            FocusPane::Input =>
                match key.code {
                    KeyCode::Enter => {
                        let text = self.textarea.lines().first().cloned().unwrap_or_default();
                        self.textarea.select_all();
                        self.textarea.cut();
                        self.last_keypress = Instant::now();
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
                            self.last_keypress = Instant::now();
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
            FocusPane::Messages =>
                match key.code {
                    KeyCode::Up => {
                        let visible = 20;
                        self.scroll_up(visible);
                    }
                    KeyCode::Down => {
                        self.scroll_down();
                    }
                    _ => {}
                }
            FocusPane::Sidebar => {
                let max = self.rooms.len().saturating_sub(1);
                match key.code {
                    KeyCode::Up => {
                        let i = self.sidebar_state.selected().unwrap_or(0);
                        self.sidebar_state.select(Some(i.saturating_sub(1)));
                    }
                    KeyCode::Down => {
                        let i = self.sidebar_state.selected().unwrap_or(0);
                        self.sidebar_state.select(Some((i + 1).min(max)));
                    }
                    KeyCode::Enter => {
                        if let Some(i) = self.sidebar_state.selected() {
                            self.select_room(i).await;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    async fn handle_help_key(&mut self, key: event::KeyEvent) {
        use crossterm::event::KeyCode;

        let max = Self::HELP_COMMANDS.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => {
                self.show_help = false;
            }
            KeyCode::Up => {
                let i = self.help_state.selected().unwrap_or(0);
                self.help_state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down => {
                let i = self.help_state.selected().unwrap_or(0);
                self.help_state.select(Some((i + 1).min(max)));
            }
            KeyCode::Enter => {
                if let Some(i) = self.help_state.selected() {
                    self.apply_help_selection(i);
                }
            }
            _ => {}
        }
    }

    async fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.show_help {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(i) = self.help_row_at(mouse.column, mouse.row) {
                        self.apply_help_selection(i);
                    } else if !Self::rect_contains(self.help_area, mouse.column, mouse.row) {
                        self.show_help = false;
                    }
                }
                MouseEventKind::ScrollUp => {
                    let i = self.help_state.selected().unwrap_or(0);
                    self.help_state.select(Some(i.saturating_sub(1)));
                }
                MouseEventKind::ScrollDown => {
                    let max = Self::HELP_COMMANDS.len().saturating_sub(1);
                    let i = self.help_state.selected().unwrap_or(0);
                    self.help_state.select(Some((i + 1).min(max)));
                }
                _ => {}
            }
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if Self::rect_contains(self.sidebar_area, mouse.column, mouse.row) {
                    self.focus = FocusPane::Sidebar;
                    if let Some(i) = self.sidebar_row_at(mouse.column, mouse.row) {
                        self.select_room(i).await;
                    }
                } else if Self::rect_contains(self.messages_area, mouse.column, mouse.row) {
                    self.focus = FocusPane::Messages;
                } else if Self::rect_contains(self.input_area, mouse.column, mouse.row) {
                    self.focus = FocusPane::Input;
                }
            }
            MouseEventKind::ScrollUp => {
                if Self::rect_contains(self.sidebar_area, mouse.column, mouse.row) {
                    let i = self.sidebar_state.selected().unwrap_or(0);
                    self.sidebar_state.select(Some(i.saturating_sub(1)));
                } else {
                    self.scroll_up(20);
                }
            }
            MouseEventKind::ScrollDown => {
                if Self::rect_contains(self.sidebar_area, mouse.column, mouse.row) {
                    let max = self.rooms.len().saturating_sub(1);
                    let i = self.sidebar_state.selected().unwrap_or(0);
                    self.sidebar_state.select(Some((i + 1).min(max)));
                } else {
                    self.scroll_down();
                }
            }
            _ => {}
        }
    }

    fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
        x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
    }

    fn sidebar_row_at(&self, x: u16, y: u16) -> Option<usize> {
        let area = self.sidebar_area;
        if
            x < area.x + 1 ||
            x >= area.x + area.width.saturating_sub(1) ||
            y < area.y + 1 ||
            y >= area.y + area.height.saturating_sub(1)
        {
            return None;
        }
        let row = ((y - (area.y + 1)) as usize) + self.sidebar_state.offset();
        (row < self.rooms.len()).then_some(row)
    }

    fn help_row_at(&self, x: u16, y: u16) -> Option<usize> {
        let area = self.help_area;
        if
            x < area.x + 1 ||
            x >= area.x + area.width.saturating_sub(1) ||
            y < area.y + 1 ||
            y >= area.y + area.height.saturating_sub(1)
        {
            return None;
        }
        let row = ((y - (area.y + 1)) as usize) + self.help_state.offset();
        (row < Self::HELP_COMMANDS.len()).then_some(row)
    }

    async fn select_room(&mut self, i: usize) {
        if let Some(room) = self.rooms.get(i).cloned() {
            self.sidebar_state.select(Some(i));
            if room != self.current_room {
                self.current_room = room.clone();
                self.scroll_offset = 0;
                self.mark_all_read(&room).await;
            }
        }
    }

    fn apply_help_selection(&mut self, i: usize) {
        if let Some((_, _, insert)) = Self::HELP_COMMANDS.get(i) {
            self.textarea.select_all();
            self.textarea.cut();
            self.textarea.insert_str(*insert);
        }
        self.show_help = false;
        self.focus = FocusPane::Input;
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
                    self.show_help = true;
                    self.help_state.select(Some(0));
                }
                "/clear" => {
                    self.messages.retain(
                        |(msg, _)| msg.room != self.current_room && !msg.room.is_empty()
                    );
                    self.scroll_offset = 0;
                }
                "/quit" => {
                    self.should_quit = true;
                    let _ = self.writer.lock().await.shutdown().await;
                }
                "/join" => {
                    let room = parts.get(1).copied().unwrap_or("").trim();
                    if room.is_empty() || room.len() > 32 || room.contains(char::is_whitespace) {
                        self.ingest_msg(
                            make_system_msg(
                                "Usage: /join <room>  (1\u{2013}32 non-whitespace chars)"
                            ),
                            true
                        );
                    } else {
                        // If a DM room with this user already exists, jump to it
                        // instead of creating a new regular room.
                        let resolved = if !room.starts_with("__dm__") {
                            let mut dm_users = vec![self.username.clone(), room.to_string()];
                            dm_users.sort();
                            let dm_room = format!("__dm__{}", dm_users.join("_"));
                            if self.rooms.iter().any(|r| r == &dm_room) {
                                dm_room
                            } else {
                                room.to_string()
                            }
                        } else {
                            room.to_string()
                        };
                        if !self.rooms.iter().any(|r| r == &resolved) {
                            self.rooms.push(resolved.clone());
                        }
                        self.current_room = resolved.clone();
                        self.mark_all_read(&resolved).await;
                        self.scroll_offset = 0;
                        self.ingest_msg(
                            make_system_msg(
                                &format!(
                                    "Joined room: {}",
                                    dm_display_name(&resolved, &self.username)
                                )
                            ),
                            true
                        );
                        let msg = format!("/join {}\n", resolved);
                        let _ = self.writer.lock().await.write_all(msg.as_bytes()).await;
                    }
                }
                "/leave" => {
                    if self.rooms.len() > 1 {
                        let left = self.current_room.clone();
                        self.rooms.retain(|r| r != &self.current_room);
                        self.current_room = self.rooms[0].clone();
                        let cur = self.current_room.clone();
                        self.clear_room_read_state(&cur);
                        self.ingest_msg(
                            make_system_msg(
                                &format!("Left room: {}", dm_display_name(&left, &self.username))
                            ),
                            true
                        );
                        let _ = self.writer.lock().await.write_all(b"/leave\n").await;
                    } else {
                        self.ingest_msg(make_system_msg("Cannot leave the last room."), true);
                    }
                }
                "/rooms" => {
                    let _ = self.writer.lock().await.write_all(b"/rooms\n").await;
                }
                "/dm" => {
                    let user = parts.get(1).copied().unwrap_or("").trim();
                    let dm_msg = parts.get(2).copied().unwrap_or("").trim();
                    if user.is_empty() || dm_msg.is_empty() {
                        self.ingest_msg(make_system_msg("Usage: /dm <user> <message>"), true);
                    } else {
                        let mut users = vec![self.username.clone(), user.to_string()];
                        users.sort();
                        let dm_room = format!("__dm__{}", users.join("_"));
                        if !self.rooms.iter().any(|r| r == &dm_room) {
                            self.rooms.push(dm_room.clone());
                        }
                        self.current_room = dm_room.clone();
                        self.mark_all_read(&dm_room).await;
                        self.scroll_offset = 0;
                        let wire = format!("/msg {} {}\n", user, dm_msg);
                        let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
                    }
                }
                _ => {
                    self.ingest_msg(make_system_msg(&format!("Unknown command: {}", text)), true);
                }
            }
        } else {
            let cur = self.current_room.clone();
            self.mark_all_read(&cur).await;
            let wire = format!("{}\n", text);
            let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
        }
    }

    async fn handle_server_message(&mut self, line: &str) {
        if line == "__CONN_CLOSED__" {
            self.ingest_msg(make_system_msg("Connection closed by server"), true);
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
                MessageType::SetActiveRoom => {
                    let room = msg.content.trim().to_string();
                    if !room.is_empty() {
                        self.current_room = room;
                    }
                }
                MessageType::ReadReceipt => {
                    for id in msg.content
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty()) {
                        self.read_message_ids.insert(id.to_string());
                    }
                }
                MessageType::UserMessage => {
                    let is_current_room = msg.room.is_empty() || msg.room == self.current_room;
                    let room = msg.room.clone();
                    let is_history = msg.is_history;
                    let ack_id = if is_current_room && !is_history && msg.username != self.username {
                        msg.id.clone()
                    } else {
                        String::new()
                    };
                    if is_history && msg.username == self.username && !msg.id.is_empty() {
                        self.read_message_ids.insert(msg.id.clone());
                    }
                    self.ingest_msg(msg, is_current_room || is_history);
                    if is_current_room && !is_history {
                        self.clear_room_read_state(&room);
                        if !ack_id.is_empty() {
                            let wire = format!("/read {} {}\n", room, ack_id);
                            let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
                        }
                    }
                }
                MessageType::TypingNotification => {
                    if msg.username != self.username {
                        let room = if msg.room.is_empty() {
                            self.current_room.clone()
                        } else {
                            msg.room.clone()
                        };
                        self.typing_users.insert(msg.username.clone(), (room, Instant::now()));
                    }
                }
                MessageType::PresenceSync => {
                    for u in msg.content
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty()) {
                        self.online_users.insert(u.to_string());
                    }
                }
                MessageType::SystemNotification => {
                    let read =
                        msg.is_history || msg.room.is_empty() || msg.room == self.current_room;
                    if !msg.is_history {
                        match msg.content.as_str() {
                            "Joined the chat" | "Joined the room" => {
                                self.online_users.insert(msg.username.clone());
                            }
                            "Left the chat" => {
                                self.online_users.remove(&msg.username);
                            }
                            _ => {}
                        }
                    }
                    self.ingest_msg(msg, read);
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
        let count = self.room_message_count(&self.current_room);
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = (self.scroll_offset + 1).min(max);
    }

    fn scroll_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    fn clamp_scroll(&mut self, visible_height: usize) {
        let count = self.room_message_count(&self.current_room);
        let max = count.saturating_sub(visible_height) as u16;
        self.scroll_offset = self.scroll_offset.min(max);
    }

    fn render_title_bar(&self, f: &mut Frame, area: Rect) {
        let pulse = |c: Color| -> Color {
            if let Color::Rgb(r, g, b) = c {
                let f = 0.6 + 0.4 * ((self.pulse_tick as f64) * 0.04).sin();
                Color::Rgb(((r as f64) * f) as u8, ((g as f64) * f) as u8, ((b as f64) * f) as u8)
            } else {
                c
            }
        };
        let title = format_title(&self.username, pulse(self.theme.primary));
        let widget = Paragraph::new(title)
            .style(Style::default())
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, area);
    }

    fn render_sidebar(&mut self, f: &mut Frame, area: Rect) {
        self.sidebar_area = area;

        // Keep the selection in range if rooms were added/removed since the
        // last frame (e.g. a fresh RoomList from the server).
        let max_select = self.rooms.len().saturating_sub(1);
        match self.sidebar_state.selected() {
            Some(i) if i > max_select => self.sidebar_state.select(Some(max_select)),
            None if !self.rooms.is_empty() => self.sidebar_state.select(Some(0)),
            _ => {}
        }

        let items: Vec<ListItem> = self.rooms
            .iter()
            .map(|room| {
                let has_unread = self.unread_rooms.contains(room);
                let active = room == &self.current_room;
                let prefix = if has_unread { "\u{25CF} " } else { "  " };
                let display = format!("{}{}", prefix, dm_display_name(room, &self.username));
                let style = if active {
                    Style::default().fg(self.theme.accent)
                } else if has_unread {
                    Style::default().fg(self.theme.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.theme.primary)
                };
                ListItem::new(
                    ratatui::text::Line::from(ratatui::text::Span::styled(display, style))
                )
            })
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(
                border_style(FocusPane::Sidebar, self.focus, self.pulse_tick, &self.theme)
            )
            .title(" Messages ");

        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(self.theme.primary)
                    .fg(self.theme.bg)
                    .add_modifier(Modifier::BOLD)
            )
            .highlight_symbol("\u{25B6} ");

        f.render_stateful_widget(list, area, &mut self.sidebar_state);
    }

    fn render_messages(&mut self, f: &mut Frame, area: Rect) {
        self.messages_area = area;
        let lines: Vec<ratatui::text::Line> = self
            .messages_for_room(&self.current_room)
            .flat_map(|(msg, _)| {
                match msg.message_type {
                    MessageType::UserMessage => {
                        let color = if msg.username == self.username {
                            if self.read_message_ids.contains(&msg.id) {
                                self.theme.success
                            } else {
                                self.theme.primary
                            }
                        } else {
                            self.theme.secondary
                        };
                        let dot_color = (
                            msg.username != self.username &&
                            self.online_users.contains(&msg.username)
                        ).then(|| username_color(&msg.username));
                        format_user_message(msg, color, self.theme.accent, dot_color)
                    }
                    MessageType::SystemNotification =>
                        format_system_message(msg, self.theme.accent),
                    MessageType::RoomList => unreachable!(),
                    MessageType::ReadReceipt => unreachable!(),
                    MessageType::PresenceSync => unreachable!(),
                    MessageType::TypingNotification => unreachable!(),
                    MessageType::SetActiveRoom => unreachable!(),
                }
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
            .border_style(
                border_style(FocusPane::Messages, self.focus, self.pulse_tick, &self.theme)
            )
            .title(format!(" #{} ", dm_display_name(&self.current_room, &self.username)));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((render_scroll, 0));
        f.render_widget(paragraph, area);
    }

    fn render_animations(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.primary));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let t = self.start_time.elapsed().as_secs_f64();

        // Cube/torus look best confined to a square-ish region (cell aspect
        // is ~1:2 w:h); the others use the full box.
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

        match self.anim_kind {
            AnimationKind::Cube => self.cube.render(f, square_area, t),
            AnimationKind::Torus => self.torus.render(f, square_area, t),
            AnimationKind::MatrixRain => self.matrix_rain.render(f, inner, t),
            AnimationKind::Starfield => self.starfield.render(f, inner, t),
            AnimationKind::Sand => self.sand.render(f, inner, t),
        }
    }

    fn render_input(&mut self, f: &mut Frame, area: Rect) {
        if self.focus == FocusPane::Input {
            self.textarea.set_cursor_style(Style::default().bg(self.theme.accent));
        } else {
            self.textarea.set_cursor_style(Style::default());
        }
        self.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(border::ROUNDED)
                .border_style(
                    border_style(FocusPane::Input, self.focus, self.pulse_tick, &self.theme)
                )
                .title(" Message ")
        );
        f.render_widget(&self.textarea, area);
    }

    fn render_status_bar(&self, f: &mut Frame, area: Rect) {
        let spark_data: Vec<u64> = self.sparkline_data
            .iter()
            .map(|v| *v as u64)
            .collect();
        let spark = Sparkline::default()
            .data(&spark_data)
            .style(Style::default().fg(self.theme.primary).bg(self.theme.bg));
        let spark_area = Rect {
            x: area.x + area.width.saturating_sub(41),
            y: area.y,
            width: (40).min(area.width.saturating_sub(1)),
            height: 1,
        };
        let unread_str = if self.unread_rooms.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = self.unread_rooms
                .iter()
                .filter(|r| *r != &self.current_room)
                .map(|r| dm_display_name(r, &self.username).to_string())
                .collect();
            if names.is_empty() {
                String::new()
            } else {
                format!(" \u{25CF}{} ", names.join(" \u{25CF}"))
            }
        };
        let typing_text = {
            let names: Vec<&str> = self.typing_users
                .iter()
                .filter(|(_, (r, _))| (r == &self.current_room || r.starts_with("__dm__")))
                .map(|(u, _)| u.as_str())
                .collect();
            if names.is_empty() {
                String::new()
            } else if names.len() == 1 {
                let dots = match (self.pulse_tick / 10) % 4 {
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
        let label = if !typing_text.is_empty() {
            typing_text
        } else if !unread_str.is_empty() {
            format!(" \u{2191}\u{2193}\u{21b5}:switch  Tab:focus {}", unread_str)
        } else {
            " \u{2191}\u{2193}\u{21b5}:switch  Tab:focus  /help:commands".to_string()
        };
        let status = Paragraph::new(label).style(Style::default().fg(Color::DarkGray));
        f.render_widget(status, area);
        if area.width >= 42 {
            f.render_widget(spark, spark_area);
        }
    }

    fn render_help_popup(&mut self, f: &mut Frame, area: Rect) {
        let mut items: Vec<ListItem> = Self::HELP_COMMANDS.iter()
            .map(|(cmd, desc, _)| {
                let line = format!("{:<18}{}", cmd, desc);
                ListItem::new(
                    ratatui::text::Line::from(
                        ratatui::text::Span::styled(line, Style::default().fg(self.theme.primary))
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
                        Style::default().fg(self.theme.accent).add_modifier(Modifier::BOLD)
                    ),
                    ratatui::text::Span::styled(
                        " switch animation",
                        Style::default().fg(self.theme.primary)
                    )
                ]
            ),
            ratatui::text::Line::from(
                vec![
                    ratatui::text::Span::styled(
                        "Ctrl+T",
                        Style::default().fg(self.theme.accent).add_modifier(Modifier::BOLD)
                    ),
                    ratatui::text::Span::styled(
                        " switch theme",
                        Style::default().fg(self.theme.primary)
                    )
                ]
            ),
        ];
        for line in &keybind_lines {
            items.push(ListItem::new(line.clone()));
        }

        let popup_w = (44u16).min(area.width.saturating_sub(2));
        let popup_h = ((Self::HELP_COMMANDS.len() as u16) + (keybind_lines.len() as u16) + 2).min(
            area.height.saturating_sub(2)
        );
        let popup_area = Rect {
            x: area.x + area.width.saturating_sub(popup_w) / 2,
            y: area.y + area.height.saturating_sub(popup_h) / 2,
            width: popup_w,
            height: popup_h,
        };
        self.help_area = popup_area;

        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.accent))
            .style(Style::default().bg(self.theme.bg))
            .title(" Commands ")
            .title_bottom(" \u{2191}\u{2193}\u{21b5} or click \u{2022} Esc to close ");

        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(self.theme.primary)
                    .fg(self.theme.bg)
                    .add_modifier(Modifier::BOLD)
            )
            .highlight_symbol("\u{25B6} ");

        f.render_stateful_widget(list, popup_area, &mut self.help_state);
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();

        if area.width < 17 || area.height < 12 {
            let msg = Paragraph::new("Terminal too small \u{2014} please resize").style(
                Style::default().fg(self.theme.primary).bg(self.theme.bg)
            );
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

        let horizontal = Layout::horizontal([Constraint::Length(16), Constraint::Min(0)]);
        let [sidebar_col, messages_area] = horizontal.areas(body_area);

        // Sidebar column: Rooms on top, small Animation box underneath.
        let sidebar_vertical = Layout::vertical([Constraint::Min(0), Constraint::Length(6)]);
        let [sidebar_area, anim_area] = sidebar_vertical.areas(sidebar_col);

        self.render_title_bar(f, title_area);
        self.render_sidebar(f, sidebar_area);
        self.render_messages(f, messages_area);
        self.render_animations(f, anim_area);
        self.input_area = input_area;
        self.render_input(f, input_area);
        self.render_status_bar(f, status_area);

        if self.show_help {
            self.render_help_popup(f, area);
        }
    }
}
