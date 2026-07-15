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
    text::Line,
    widgets::{ Block, Borders, Clear, List, ListItem, ListState, Paragraph, Sparkline, Wrap },
};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{ Image as RatatuiImage, Resize };
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
    format_image_header,
    format_audio_message,
    make_system_msg,
    username_color,
};
use super::anims::{ SpinningCube, MatrixRain, Starfield, SpinningTorus, SandSim };
use super::types::{ AnimationKind, FocusPane, Theme, THEMES };

pub async fn run_chat_ui(
    username: String,
    token: String,
    reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
    writer: tokio::io::WriteHalf<ClientStream>
) -> Result<(), Box<dyn std::error::Error>> {
    let (server_tx, server_rx) = mpsc::unbounded_channel::<String>();
    let mut app = App::new(username, token, writer, server_rx, server_tx.clone());
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

#[derive(serde::Deserialize)]
struct UploadResponse {
    url: String,
    thumb_url: String,
    width: u32,
    height: u32,
}

#[derive(serde::Deserialize)]
struct AudioUploadResponse {
    url: String,
    duration_ms: u32,
}

async fn do_image_upload(path: &str, token: &str) -> Result<UploadResponse, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("cannot read file: {}", e))?;

    let file_name = std::path::Path
        ::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "upload.png".to_string());

    let part = reqwest::multipart::Part
        ::bytes(bytes)
        .file_name(file_name)
        .mime_str("application/octet-stream")
        .map_err(|e| e.to_string())?;

    let form = reqwest::multipart::Form::new().text("token", token.to_string()).part("file", part);

    let upload_base = std::env
        ::var("UPLOAD_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8083".to_string());

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/upload?token={}", upload_base, token))
        .multipart(form)
        .send().await
        .map_err(|e| format!("upload request failed: {}", e))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("failed to read response: {}", e))?;

    if !status.is_success() {
        return Err(format!("upload failed ({}): {}", status, body));
    }

    serde_json::from_str::<UploadResponse>(&body).map_err(|e| format!("invalid response: {}", e))
}

async fn do_audio_upload(path: &str, token: &str) -> Result<AudioUploadResponse, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("cannot read file: {}", e))?;

    let file_name = std::path::Path
        ::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "upload.wav".to_string());

    let part = reqwest::multipart::Part
        ::bytes(bytes)
        .file_name(file_name)
        .mime_str("application/octet-stream")
        .map_err(|e| e.to_string())?;

    let form = reqwest::multipart::Form::new().text("token", token.to_string()).part("file", part);

    let upload_base = std::env
        ::var("UPLOAD_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8083".to_string());

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/upload/audio?token={}", upload_base, token))
        .multipart(form)
        .send().await
        .map_err(|e| format!("upload request failed: {}", e))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("failed to read response: {}", e))?;

    if !status.is_success() {
        return Err(format!("upload failed ({}): {}", status, body));
    }

    serde_json
        ::from_str::<AudioUploadResponse>(&body)
        .map_err(|e| format!("invalid response: {}", e))
}

pub struct App {
    pub username: String,
    pub token: String,
    pub rooms: Vec<String>,
    pub current_room: String,
    messages: Vec<(ChatMessage, bool)>,
    pub scroll_offset: u16,
    pub focus: FocusPane,
    pub textarea: TextArea<'static>,
    pub writer: Arc<Mutex<tokio::io::WriteHalf<ClientStream>>>,
    pub should_quit: bool,
    pub server_rx: mpsc::UnboundedReceiver<String>,
    server_tx: mpsc::UnboundedSender<String>,
    sidebar_state: ListState,
    sidebar_area: Rect,
    messages_area: Rect,
    input_area: Rect,
    show_help: bool,
    help_state: ListState,
    help_area: Rect,
    unread_rooms: HashSet<String>,
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

    // Image rendering
    picker: Picker,
    image_cache: HashMap<String, Protocol>,
    inflight_images: HashSet<String>,
    image_rx: mpsc::UnboundedReceiver<(String, Protocol)>,
    image_results_tx: mpsc::UnboundedSender<(String, Protocol)>,
    image_cell_height: u16,
    image_cell_width: u16,
    last_resize: Instant,
    dirty: bool,

    // Audio recording
    is_recording: bool,
    record_start: Option<Instant>,

    // Audio playback
    playing_audio: Option<String>, // message id currently playing
}

impl App {
    /// (display command, description, text inserted into the input box
    /// when picked from the `/help` popup). Argument-taking commands insert
    /// a trailing space so the cursor lands ready to type the argument.
    const HELP_COMMANDS: [(&'static str, &'static str, &'static str); 10] = [
        ("/join <room>", "join a room", "/join "),
        ("/leave", "leave current room", "/leave"),
        ("/rooms", "list server rooms", "/rooms"),
        ("/dm <user> <msg>", "send a direct message", "/dm "),
        ("/image <path>", "share an image", "/image "),
        ("/audio <path>", "send an audio file", "/audio "),
        ("/record", "toggle audio recording", "/record"),
        ("/clear", "clear messages", "/clear"),
        ("/help", "show this help", "/help"),
        ("/quit", "quit", "/quit"),
    ];

    fn new(
        username: String,
        token: String,
        writer: tokio::io::WriteHalf<ClientStream>,
        server_rx: mpsc::UnboundedReceiver<String>,
        server_tx: mpsc::UnboundedSender<String>
    ) -> Self {
        let default_theme = &THEMES[0];
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        ta.set_style(Style::default().fg(default_theme.primary).bg(default_theme.bg));
        ta.set_cursor_style(Style::default().bg(default_theme.primary));

        let (image_results_tx, image_rx) = mpsc::unbounded_channel();

        let mut app = Self {
            username,
            token,
            rooms: vec!["general".to_string()],
            current_room: "general".to_string(),
            focus: FocusPane::Input,
            messages: Vec::new(),
            scroll_offset: 0,
            should_quit: false,
            textarea: ta,
            writer: Arc::new(Mutex::new(writer)),
            server_rx,
            server_tx,
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
            picker: Picker::halfblocks(),
            image_cache: HashMap::new(),
            inflight_images: HashSet::new(),
            image_rx,
            image_results_tx,
            image_cell_height: 8,
            image_cell_width: 16,
            last_resize: Instant::now(),
            dirty: true,
            is_recording: false,
            record_start: None,
            playing_audio: None,
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

        // Everyday I am reminded why i hate windows (* ￣︿￣)
        self.picker = Picker::halfblocks(); // <---- I couldn't for my life get this working, so i had to use this
        self.update_image_cell_size();

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
        let mut last_anim_tick = Instant::now();
        let anim_interval = Duration::from_millis(16);
        loop {
            let now = Instant::now();
            let anim_tick = now.duration_since(last_anim_tick) >= anim_interval;

            if anim_tick {
                self.pulse_tick = self.pulse_tick.wrapping_add(1);
                last_anim_tick = Instant::now();
                self.dirty = true;
            }

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
                self.dirty = true;
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
            let had_typing = !self.typing_users.is_empty();
            self.typing_users.retain(|_, (_, t)| now - *t < Duration::from_secs(4));
            if had_typing != !self.typing_users.is_empty() {
                self.dirty = true;
            }

            if event::poll(Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            self.handle_key(key).await;
                            self.dirty = true;
                        }
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse(mouse).await;
                        self.dirty = true;
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
                                self.dirty = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            loop {
                match self.server_rx.try_recv() {
                    Ok(line) => {
                        self.handle_server_message(&line).await;
                        self.dirty = true;
                    }
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
            loop {
                match self.image_rx.try_recv() {
                    Ok((id, proto)) => {
                        self.image_cache.insert(id, proto);
                        self.dirty = true;
                    }
                    Err(mpsc::error::TryRecvError::Empty) => {
                        break;
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        break;
                    }
                }
            }
            if self.should_quit {
                break;
            }

            if self.dirty {
                terminal.draw(|f| self.render(f))?;
                self.dirty = false;
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
        self.dirty = true;
        let visible = self.messages_area.height.saturating_sub(2) as usize;
        let visible = if visible == 0 { 20 } else { visible };
        self.clamp_scroll(visible);
    }

    /// Locally resets any lingering "unread" color for messages in the room
    /// back to normal, without notifying anyone
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

    /// Ids of currently-unread messages written by someone else in the room,
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

    /// Marks everything in the room as read locally and tells the server, so
    /// the original senders can see their messages flip to the "read"
    /// color.
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
                        let visible = self.messages_area.height.saturating_sub(2) as usize;
                        self.scroll_up(visible);
                    }
                    KeyCode::Down => {
                        self.scroll_down();
                    }
                    KeyCode::Enter | KeyCode::Char('p') => {
                        self.toggle_play_audio_in_room();
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
                    let visible = self.messages_area.height.saturating_sub(2) as usize;
                    self.scroll_up(visible);
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
                let wire = format!("/switch {}\n", room);
                let _ = self.writer.lock().await.write_all(wire.as_bytes()).await;
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
                "/image" => {
                    let path = parts.get(1).copied().unwrap_or("").trim();
                    if path.is_empty() {
                        self.ingest_msg(make_system_msg("Usage: /image <file_path>"), true);
                    } else {
                        let path = path.to_string();
                        let token = self.token.clone();
                        let writer = self.writer.clone();
                        let err_tx = self.server_tx.clone();
                        self.ingest_msg(make_system_msg("Uploading image..."), true);
                        tokio::spawn(async move {
                            match do_image_upload(&path, &token).await {
                                Ok(resp) => {
                                    let wire = format!(
                                        "/image {} {} {} {}\n",
                                        resp.url,
                                        resp.thumb_url,
                                        resp.width,
                                        resp.height
                                    );
                                    let _ = writer.lock().await.write_all(wire.as_bytes()).await;
                                }
                                Err(e) => {
                                    let _ = err_tx.send(format!("Image upload failed: {}", e));
                                }
                            }
                        });
                    }
                }
                "/audio" => {
                    let path = parts.get(1).copied().unwrap_or("").trim();
                    if path.is_empty() {
                        self.ingest_msg(make_system_msg("Usage: /audio <file_path>"), true);
                    } else {
                        let path = path.to_string();
                        let token = self.token.clone();
                        let writer = self.writer.clone();
                        let err_tx = self.server_tx.clone();
                        self.ingest_msg(make_system_msg("Uploading audio..."), true);
                        tokio::spawn(async move {
                            match do_audio_upload(&path, &token).await {
                                Ok(resp) => {
                                    let wire = format!(
                                        "/audio {} {}\n",
                                        resp.url,
                                        resp.duration_ms
                                    );
                                    let _ = writer.lock().await.write_all(wire.as_bytes()).await;
                                }
                                Err(e) => {
                                    let _ = err_tx.send(format!("Audio upload failed: {}", e));
                                }
                            }
                        });
                    }
                }
                "/record" => {
                    if self.is_recording {
                        // Stop recording
                        self.is_recording = false;
                        let elapsed = self.record_start
                            .map(|s| s.elapsed().as_millis() as u32)
                            .unwrap_or(0);
                        self.record_start = None;
                        self.ingest_msg(
                            make_system_msg(
                                &format!(
                                    "Recording stopped ({:.1}s). Encoding and uploading...",
                                    (elapsed as f64) / 1000.0
                                )
                            ),
                            true
                        );

                        let token = self.token.clone();
                        let writer = self.writer.clone();
                        let err_tx = self.server_tx.clone();
                        let duration_ms = elapsed;

                        tokio::task::spawn_blocking(move || {
                            use rodio::Source;
                            use rodio::microphone::MicrophoneBuilder;

                            let result = (|| -> Result<(), String> {
                                let mic = MicrophoneBuilder::new()
                                    .default_device()
                                    .map_err(|e| format!("No input device: {}", e))?
                                    .default_config()
                                    .map_err(|e| format!("Mic config error: {}", e))?
                                    .open_stream()
                                    .map_err(|e| format!("Mic open error: {}", e))?;

                                let sample_rate = mic.sample_rate();
                                let channels = mic.channels();

                                let recording = mic
                                    .take_duration(
                                        std::time::Duration::from_millis(duration_ms as u64)
                                    )
                                    .record();

                                let mut samples: Vec<f32> = Vec::new();
                                for s in recording {
                                    samples.push(s);
                                }

                                if samples.is_empty() {
                                    return Err("No audio captured".to_string());
                                }

                                // Write WAV to temp file
                                let wav_path = std::env::temp_dir().join("retro_audio_record.wav");
                                let spec = hound::WavSpec {
                                    channels: channels.get(),
                                    sample_rate: sample_rate.get(),
                                    bits_per_sample: 16,
                                    sample_format: hound::SampleFormat::Int,
                                };
                                let mut wav_writer = hound::WavWriter
                                    ::create(&wav_path, spec)
                                    .map_err(|e| format!("WAV create error: {}", e))?;
                                for &s in &samples {
                                    let sample_i16 = (s.clamp(-1.0, 1.0) *
                                        (i16::MAX as f32)) as i16;
                                    wav_writer
                                        .write_sample(sample_i16)
                                        .map_err(|e| format!("WAV write error: {}", e))?;
                                }
                                wav_writer
                                    .finalize()
                                    .map_err(|e| format!("WAV finalize error: {}", e))?;

                                // Upload via async runtime
                                let rt = tokio::runtime::Handle::current();
                                rt.block_on(async move {
                                    let path_str = wav_path.to_string_lossy().to_string();
                                    match do_audio_upload(&path_str, &token).await {
                                        Ok(resp) => {
                                            let wire = format!(
                                                "/audio {} {}\n",
                                                resp.url,
                                                resp.duration_ms
                                            );
                                            let _ = writer
                                                .lock().await
                                                .write_all(wire.as_bytes()).await;
                                        }
                                        Err(e) => {
                                            let _ = err_tx.send(
                                                format!("Audio upload failed: {}", e)
                                            );
                                        }
                                    }
                                });
                                Ok(())
                            })();

                            if let Err(e) = result {
                                let _ = err_tx.send(e);
                            }
                        });
                    } else {
                        // Start recording
                        self.is_recording = true;
                        self.record_start = Some(Instant::now());
                        self.ingest_msg(
                            make_system_msg("Recording audio... (type /record again to stop)"),
                            true
                        );
                    }
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
                MessageType::ImageMessage => {
                    let is_current_room = msg.room.is_empty() || msg.room == self.current_room;
                    let room = msg.room.clone();
                    let is_history = msg.is_history;
                    if is_history && msg.username == self.username && !msg.id.is_empty() {
                        self.read_message_ids.insert(msg.id.clone());
                    }
                    self.ingest_msg(msg, is_current_room || is_history);
                    if is_current_room && !is_history {
                        self.clear_room_read_state(&room);
                    }
                }
                MessageType::AudioMessage => {
                    let is_current_room = msg.room.is_empty() || msg.room == self.current_room;
                    let room = msg.room.clone();
                    let is_history = msg.is_history;
                    if is_history && msg.username == self.username && !msg.id.is_empty() {
                        self.read_message_ids.insert(msg.id.clone());
                    }
                    self.ingest_msg(msg, is_current_room || is_history);
                    if is_current_room && !is_history {
                        self.clear_room_read_state(&room);
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
        } else if !line.is_empty() {
            self.ingest_msg(make_system_msg(line), true);
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
        let content_width = self.messages_area.width.saturating_sub(2) as usize;
        let total = self.total_content_height(content_width as u16);
        let max = total.saturating_sub(visible_height as u16) as u16;
        self.scroll_offset = (self.scroll_offset + 1).min(max);
    }

    fn scroll_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    fn clamp_scroll(&mut self, visible_height: usize) {
        let content_width = self.messages_area.width.saturating_sub(2) as usize;
        let total = self.total_content_height(content_width as u16);
        let max = total.saturating_sub(visible_height as u16) as u16;
        self.scroll_offset = self.scroll_offset.min(max);
    }

    fn update_image_cell_size(&mut self) {
        let (fw, fh) = self.picker.font_size();
        let thumb_px = 128u32;
        self.image_cell_height = (thumb_px / (fh as u32)).max(1) as u16;
        self.image_cell_width = (thumb_px / (fw as u32)).max(1) as u16;
    }

    fn message_line_height(&self, msg: &ChatMessage, content_width: u16) -> u16 {
        match msg.message_type {
            MessageType::ImageMessage => {
                let header_lines = 1u16;
                header_lines + self.image_cell_height
            }
            MessageType::AudioMessage => 1,
            MessageType::SystemNotification => {
                if msg.content.is_empty() { 1 } else { msg.content.lines().count() as u16 }
            }
            MessageType::UserMessage => {
                let ts_len = 5usize;
                let user_len = msg.username.len();
                let overhead = ts_len + user_len + 5;
                let wrap_width = content_width as usize;
                if wrap_width == 0 {
                    return 1;
                }
                let lines: Vec<&str> = msg.content.lines().collect();
                if lines.is_empty() {
                    return 1;
                }
                let mut total = 0u16;
                for line in &lines {
                    let line_chars = line.chars().count();
                    if line_chars == 0 {
                        total += 1;
                    } else {
                        let first_wrap = wrap_width.saturating_sub(overhead);
                        if first_wrap == 0 {
                            total += 1;
                        } else if line_chars <= first_wrap {
                            total += 1;
                        } else {
                            total += 1;
                            let remaining = line_chars - first_wrap;
                            total +=
                                ((remaining as u16) + (wrap_width as u16) - 1) /
                                (wrap_width as u16);
                        }
                    }
                }
                total.max(1)
            }
            _ => 1,
        }
    }

    fn total_content_height(&self, content_width: u16) -> u16 {
        self.messages_for_room(&self.current_room)
            .map(|(msg, _)| self.message_line_height(msg, content_width))
            .sum()
    }

    fn rebuild_image_protocols(&mut self) {
        let old_cache = std::mem::take(&mut self.image_cache);
        for (id, _old_proto) in old_cache {
            if let Some(msg) = self.messages.iter().find(|(m, _)| m.id == id) {
                let msg = &msg.0;
                if !msg.thumb_url.is_empty() {
                    self.inflight_images.remove(&id);
                    self.spawn_image_fetch(id, msg.thumb_url.clone());
                }
            }
        }
    }

    fn spawn_image_fetch(&self, msg_id: String, thumb_url: String) {
        let tx = self.image_results_tx.clone();
        let picker = self.picker.clone();
        let upload_base = std::env
            ::var("UPLOAD_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8083".to_string());
        let url = format!("{}{}", upload_base, thumb_url);
        let cell_w = self.image_cell_width;
        let cell_h = self.image_cell_height;

        tokio::spawn(async move {
            let Ok(resp) = reqwest::get(&url).await else {
                return;
            };
            let Ok(bytes) = resp.bytes().await else {
                return;
            };

            let proto = tokio::task
                ::spawn_blocking(move || {
                    let img = image::load_from_memory(&bytes).ok()?;
                    let rect = ratatui::layout::Rect::new(0, 0, cell_w, cell_h);
                    picker.new_protocol(img, rect, Resize::Fit(None)).ok()
                }).await
                .ok()
                .flatten();

            if let Some(proto) = proto {
                let _ = tx.send((msg_id, proto));
            }
        });
    }

    fn toggle_play_audio_in_room(&mut self) {
        if let Some(ref playing_id) = self.playing_audio.clone() {
            self.playing_audio = None;
            self.ingest_msg(make_system_msg("Stopped playback."), true);
            return;
        }

        // Find the most recent AudioMessage in the current room
        let audio_msg = self
            .messages_for_room(&self.current_room)
            .rev()
            .find(
                |(msg, _)|
                    msg.message_type == MessageType::AudioMessage && !msg.audio_url.is_empty()
            )
            .map(|(msg, _)| msg.clone());

        if let Some(msg) = audio_msg {
            let msg_id = msg.id.clone();
            let audio_url = msg.audio_url.clone();
            let upload_base = std::env
                ::var("UPLOAD_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8083".to_string());
            let full_url = format!("{}{}", upload_base, audio_url);
            let err_tx = self.server_tx.clone();
            let playing_id = msg_id.clone();

            self.playing_audio = Some(playing_id);
            self.ingest_msg(make_system_msg("Playing audio..."), true);

            tokio::spawn(async move {
                match reqwest::get(&full_url).await {
                    Ok(resp) => {
                        match resp.bytes().await {
                            Ok(bytes) => {
                                let _ = tokio::task::spawn_blocking(move || {
                                    let cursor = std::io::Cursor::new(bytes.to_vec());
                                    match rodio::Decoder::new(cursor) {
                                        Ok(decoder) => {
                                            let (_stream, handle) = rodio::OutputStream
                                                ::try_default()
                                                .unwrap();
                                            let sink = rodio::Sink::try_new(&handle).unwrap();
                                            sink.append(decoder);
                                            sink.sleep_until_end();
                                        }
                                        Err(e) => {
                                            let _ = err_tx.send(format!("Decode error: {}", e));
                                        }
                                    }
                                }).await;
                            }
                            Err(e) => {
                                let _ = err_tx.send(format!("Audio download error: {}", e));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = err_tx.send(format!("Audio fetch error: {}", e));
                    }
                }
            });
        }
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

        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(
                border_style(FocusPane::Messages, self.focus, self.pulse_tick, &self.theme)
            )
            .title(format!(" #{} ", dm_display_name(&self.current_room, &self.username)));
        let content_area = block.inner(area);
        f.render_widget(block, area);

        if content_area.height == 0 || content_area.width == 0 {
            return;
        }

        let room_msgs: Vec<_> = self.messages_for_room(&self.current_room).cloned().collect();
        if room_msgs.is_empty() {
            return;
        }

        let content_width = content_area.width as usize;
        let mut msg_heights: Vec<u16> = Vec::with_capacity(room_msgs.len());
        let mut total_height = 0u16;
        for (msg, _) in &room_msgs {
            let h = self.message_line_height(msg, content_width as u16);
            msg_heights.push(h);
            total_height = total_height.saturating_add(h);
        }

        let visible_height = content_area.height;
        let max_scroll = total_height.saturating_sub(visible_height);
        let render_scroll = if self.scroll_offset == 0 {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        };

        let vis_start = total_height.saturating_sub(render_scroll + visible_height);
        let vis_end = total_height.saturating_sub(render_scroll);

        let mut current_y = 0u16;
        for (i, (msg, _)) in room_msgs.iter().enumerate() {
            let h = msg_heights[i];
            let msg_top = current_y;
            let msg_bottom = current_y.saturating_add(h);

            if msg_bottom > vis_start && msg_top < vis_end {
                let screen_y = content_area.y + msg_top.saturating_sub(vis_start);
                let max_h = content_area.height.saturating_sub(screen_y - content_area.y);
                let msg_area = Rect {
                    x: content_area.x,
                    y: screen_y,
                    width: content_area.width,
                    height: h.min(max_h),
                };

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
                        let lines = format_user_message(msg, color, self.theme.accent, dot_color);
                        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
                        f.render_widget(para, msg_area);
                    }
                    MessageType::SystemNotification => {
                        let lines = format_system_message(msg, self.theme.accent);
                        let para = Paragraph::new(lines);
                        f.render_widget(para, msg_area);
                    }
                    MessageType::ImageMessage => {
                        let color = if msg.username == self.username {
                            self.theme.primary
                        } else {
                            self.theme.secondary
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

                        if let Some(protocol) = self.image_cache.get(&msg.id) {
                            let img_widget = RatatuiImage::new(protocol);
                            f.render_widget(img_widget, img_area);
                        } else {
                            if !self.inflight_images.contains(&msg.id) && !msg.id.is_empty() {
                                self.inflight_images.insert(msg.id.clone());
                                self.spawn_image_fetch(msg.id.clone(), msg.thumb_url.clone());
                            }
                            let placeholder = Paragraph::new(
                                Line::from(
                                    ratatui::text::Span::styled(
                                        "  \u{2593}\u{2593} image \u{2593}\u{2593}",
                                        Style::default().fg(self.theme.accent)
                                    )
                                )
                            );
                            f.render_widget(placeholder, img_area);
                        }
                    }
                    MessageType::AudioMessage => {
                        let color = if msg.username == self.username {
                            self.theme.primary
                        } else {
                            self.theme.secondary
                        };
                        let is_playing = self.playing_audio.as_deref() == Some(&msg.id);
                        let lines = format_audio_message(msg, color, is_playing);
                        let para = Paragraph::new(lines);
                        f.render_widget(para, msg_area);
                    }
                    _ => {}
                }
            }

            current_y = msg_bottom;
        }
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
        let label = if self.is_recording {
            let elapsed = self.record_start.map(|s| s.elapsed().as_secs()).unwrap_or(0);
            let dots = match (self.pulse_tick / 8) % 4 {
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
        let status = if self.is_recording {
            Paragraph::new(label).style(
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            )
        } else {
            Paragraph::new(label).style(Style::default().fg(Color::DarkGray))
        };
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

        let sidebar_vertical = Layout::vertical([Constraint::Min(0), Constraint::Length(6)]);
        let [sidebar_area, anim_area] = sidebar_vertical.areas(sidebar_col);

        let now = Instant::now();
        if now.duration_since(self.last_resize) > Duration::from_millis(300) {
            let old_h = self.image_cell_height;
            self.update_image_cell_size();
            if self.image_cell_height != old_h && !self.image_cache.is_empty() {
                self.rebuild_image_protocols();
            }
            self.last_resize = now;
        }

        self.render_title_bar(f, title_area);
        self.render_sidebar(f, sidebar_area);
        self.render_animations(f, anim_area);
        self.input_area = input_area;
        self.render_input(f, input_area);
        self.render_status_bar(f, status_area);
        self.render_messages(f, messages_area);

        if self.show_help {
            self.render_help_popup(f, area);
        }
    }
}
