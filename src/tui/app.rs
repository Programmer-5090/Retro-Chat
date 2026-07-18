use std::{
    collections::{ HashMap, HashSet, VecDeque },
    sync::{ Arc, atomic::AtomicBool },
    time::{ Duration, Instant },
};

use crossterm::{
    event::{
        self,
        DisableBracketedPaste,
        DisableMouseCapture,
        EnableBracketedPaste,
        EnableMouseCapture,
        Event,
        KeyEventKind,
    },
    execute,
    terminal::{ EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode },
};
use ratatui::{
    Frame,
    layout::{ Constraint, Layout, Rect },
    prelude::{ CrosstermBackend, Terminal },
    style::Style,
    widgets::{ ListState, Paragraph },
};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use tokio::{ io::{ AsyncBufReadExt, AsyncWriteExt, BufReader }, sync::{ Mutex, mpsc } };
use tui_textarea::TextArea;

use crate::client_helpers::ClientStream;
use crate::ChatMessage;

use super::format::make_system_msg;
use super::anims::{ SpinningCube, MatrixRain, Starfield, SpinningTorus, SandSim };
use super::types::{ AnimationKind, FocusPane, Theme, THEMES };

struct CleanGuard;

impl Drop for CleanGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste);
    }
}

pub struct AudioState {
    pub is_recording: bool,
    pub push_to_talk_active: bool,
    pub record_start: Option<Instant>,
    pub record_stream: Option<cpal::Stream>,
    pub record_samples: Option<Arc<std::sync::Mutex<Vec<f32>>>>,
    pub record_channels: Option<std::num::NonZeroU16>,
    pub record_sample_rate: Option<std::num::NonZeroU32>,
    pub playing_audio: Option<String>,
    pub live_spectrum: Vec<f32>,
    pub spectrum_tx: mpsc::UnboundedSender<(String, Vec<f32>)>,
    pub spectrum_rx: mpsc::UnboundedReceiver<(String, Vec<f32>)>,
    pub spectrum_stop: Option<Arc<AtomicBool>>,
}

pub struct ImageState {
    pub picker: Picker,
    pub image_cache: HashMap<String, Protocol>,
    pub inflight_images: HashSet<String>,
    pub image_rx: mpsc::UnboundedReceiver<(String, Protocol)>,
    pub image_results_tx: mpsc::UnboundedSender<(String, Protocol)>,
    pub image_cell_height: u16,
    pub image_cell_width: u16,
    pub last_resize: Instant,
}

pub struct InputState {
    pub textarea: TextArea<'static>,
    pub focus: FocusPane,
    pub show_help: bool,
    pub help_state: ListState,
    pub help_area: Rect,
}

pub struct UiState {
    pub pulse_tick: u64,
    pub theme: Theme,
    pub theme_idx: usize,
    pub sidebar_state: ListState,
    pub sidebar_area: Rect,
    pub messages_area: Rect,
    pub input_area: Rect,
    pub cube: SpinningCube,
    pub matrix_rain: MatrixRain,
    pub starfield: Starfield,
    pub torus: SpinningTorus,
    pub sand: SandSim,
    pub anim_kind: AnimationKind,
    pub start_time: Instant,
    pub sparkline_data: VecDeque<u16>,
}

pub struct ReadState {
    pub unread_rooms: HashSet<String>,
    pub read_message_ids: HashSet<String>,
}

pub struct TypingState {
    pub typing_users: HashMap<String, (String, Instant)>,
    pub last_typing_sent: Instant,
}

pub struct App {
    pub username: String,
    pub token: String,
    pub rooms: Vec<String>,
    pub current_room: String,
    pub(crate) messages: Vec<(ChatMessage, bool)>,
    pub scroll_offset: u16,
    pub writer: Arc<Mutex<tokio::io::WriteHalf<ClientStream>>>,
    pub should_quit: bool,
    pub server_rx: mpsc::UnboundedReceiver<String>,
    pub(crate) server_tx: mpsc::UnboundedSender<String>,
    pub last_keypress: Instant,
    pub(crate) dirty: bool,
    pub online_users: HashSet<String>,
    pub(crate) message_times: VecDeque<Instant>,
    pub(crate) system_expiry: HashMap<String, Instant>,

    pub input: InputState,
    pub audio: AudioState,
    pub images: ImageState,
    pub ui: UiState,
    pub read: ReadState,
    pub typing: TypingState,
}

impl App {
    fn new(
        username: String,
        token: String,
        writer: tokio::io::WriteHalf<ClientStream>,
        server_rx: mpsc::UnboundedReceiver<String>,
        server_tx: mpsc::UnboundedSender<String>
    ) -> Self {
        let (image_results_tx, image_rx) = mpsc::unbounded_channel();
        let (spectrum_tx, spectrum_rx) = mpsc::unbounded_channel();

        let default_theme = &THEMES[0];
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        ta.set_style(Style::default().fg(default_theme.primary).bg(default_theme.bg));
        ta.set_cursor_style(Style::default().bg(default_theme.primary));

        let input = InputState {
            textarea: ta,
            focus: FocusPane::Input,
            show_help: false,
            help_state: ListState::default().with_selected(Some(0)),
            help_area: Rect::default(),
        };

        let audio = AudioState {
            is_recording: false,
            push_to_talk_active: false,
            record_start: None,
            record_stream: None,
            record_samples: None,
            record_channels: None,
            record_sample_rate: None,
            playing_audio: None,
            live_spectrum: Vec::new(),
            spectrum_tx,
            spectrum_rx,
            spectrum_stop: None,
        };

        let images = ImageState {
            picker: Picker::halfblocks(),
            image_cache: HashMap::new(),
            inflight_images: HashSet::new(),
            image_rx,
            image_results_tx,
            image_cell_height: 0,
            image_cell_width: 0,
            last_resize: Instant::now(),
        };

        let ui = UiState {
            pulse_tick: 0,
            theme: *default_theme,
            theme_idx: 0,
            sidebar_state: ListState::default().with_selected(Some(0)),
            sidebar_area: Rect::default(),
            messages_area: Rect::default(),
            input_area: Rect::default(),
            cube: SpinningCube::new(),
            matrix_rain: MatrixRain::new(),
            starfield: Starfield::new(),
            torus: SpinningTorus::new(),
            sand: SandSim::new(),
            anim_kind: AnimationKind::Cube,
            start_time: Instant::now(),
            sparkline_data: VecDeque::new(),
        };

        let read = ReadState {
            unread_rooms: HashSet::new(),
            read_message_ids: HashSet::new(),
        };

        let typing = TypingState {
            typing_users: HashMap::new(),
            last_typing_sent: Instant::now(),
        };

        let mut app = Self {
            username,
            token,
            rooms: vec!["general".to_string()],
            server_rx,
            server_tx,
            current_room: "general".to_string(),
            messages: Vec::new(),
            scroll_offset: 0,
            writer: Arc::new(Mutex::new(writer)),
            should_quit: false,
            last_keypress: Instant::now(),
            dirty: false,
            online_users: HashSet::new(),
            message_times: VecDeque::new(),
            system_expiry: HashMap::new(),
            input,
            audio,
            images,
            ui,
            read,
            typing,
        };

        app.ui.cube.color = default_theme.primary;
        app.ui.matrix_rain.color = default_theme.primary;
        app.ui.starfield.color = default_theme.primary;
        app.ui.torus.color = default_theme.primary;
        app.ui.sand.color = default_theme.primary;
        app
    }

    async fn run(
        &mut self,
        reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
        server_tx: mpsc::UnboundedSender<String>
    ) -> Result<(), Box<dyn std::error::Error>> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        let _guard = CleanGuard;

        // Everyday I am reminded why i hate windows (* ￣︿￣)
        self.images.picker = Picker::halfblocks(); // <---- I couldn't for my life get this working, so i had to use this
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
        let mut last_expiry_cleanup = Instant::now();
        let anim_interval = Duration::from_millis(16);
        loop {
            let now = Instant::now();
            let anim_tick = now.duration_since(last_anim_tick) >= anim_interval;

            if anim_tick {
                self.ui.pulse_tick = self.ui.pulse_tick.wrapping_add(1);
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
                self.ui.sparkline_data.push_back(count);
                self.ui.sparkline_data.pop_front();
                last_sparkline_tick = now;
                self.dirty = true;
            }

            if now - last_expiry_cleanup >= Duration::from_millis(200) {
                super::server_msg::cleanup_sys_messages(self);
                last_expiry_cleanup = now;
            }

            // Typing indicator debounce
            let now = Instant::now();
            let since_keypress = now - self.last_keypress;
            let since_typing_sent = now - self.typing.last_typing_sent;
            let input_text = self.input.textarea.lines().first().cloned().unwrap_or_default();
            if
                self.input.focus == FocusPane::Input &&
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
                    self.typing.last_typing_sent = now;
                }
            }
            // Clean stale typing indicators
            let had_typing = !self.typing.typing_users.is_empty();
            self.typing.typing_users.retain(|_, (_, t)| now - *t < Duration::from_secs(4));
            if had_typing != !self.typing.typing_users.is_empty() {
                self.dirty = true;
            }

            if event::poll(Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            super::input::handle_key(self, key).await;
                            self.dirty = true;
                        } else if key.kind == KeyEventKind::Release {
                            super::input::handle_key_release(self, key).await;
                            self.dirty = true;
                        }
                    }
                    Event::Mouse(mouse) => {
                        super::input::handle_mouse(self, mouse).await;
                        self.dirty = true;
                    }
                    Event::Paste(data) => {
                        if self.input.focus == FocusPane::Input {
                            let text = data.lines().next().unwrap_or(&data);
                            let current_len = self.input.textarea
                                .lines()
                                .first()
                                .map(|l| l.len())
                                .unwrap_or(0);
                            let paste_len = text.len();
                            if current_len + paste_len <= 500 {
                                self.input.textarea.insert_str(text);
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
                        super::server_msg::handle_server_message(self, &line).await;
                        self.dirty = true;
                    }
                    Err(mpsc::error::TryRecvError::Empty) => {
                        break;
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        super::server_msg::ingest_msg(
                            self,
                            make_system_msg("Internal channel error"),
                            true
                        );
                        self.should_quit = true;
                        break;
                    }
                }
            }
            loop {
                match self.images.image_rx.try_recv() {
                    Ok((id, proto)) => {
                        self.images.image_cache.insert(id, proto);
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
            loop {
                match self.audio.spectrum_rx.try_recv() {
                    Ok((id, bins)) => {
                        if self.audio.playing_audio.as_deref() == Some(id.as_str()) {
                            self.audio.live_spectrum = bins;
                            self.dirty = true;
                        }
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

    pub(crate) fn ingest_msg(&mut self, msg: ChatMessage, read: bool) {
        super::server_msg::ingest_msg(self, msg, read);
    }

    pub(crate) async fn mark_all_read(&mut self, room: &str) {
        super::server_msg::mark_all_read(self, room).await;
    }

    pub(crate) fn clear_room_read_state(&mut self, room: &str) {
        super::server_msg::clear_room_read_state(self, room);
    }

    fn update_image_cell_size(&mut self) {
        let (fw, fh) = self.images.picker.font_size();
        let thumb_px = 128u32;
        self.images.image_cell_height = (thumb_px / (fh as u32)).max(1) as u16;
        self.images.image_cell_width = (thumb_px / (fw as u32)).max(1) as u16;
    }

    pub(crate) fn message_line_height(&self, msg: &ChatMessage, content_width: u16) -> u16 {
        super::server_msg::message_line_height(self, msg, content_width)
    }

    pub(crate) fn messages_for_room<'a>(
        &'a self,
        room: &'a str
    ) -> impl Iterator<Item = &'a (ChatMessage, bool)> {
        super::server_msg::messages_for_room(self, room)
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();

        if area.width < 17 || area.height < 12 {
            let msg = Paragraph::new("Terminal too small \u{2014} please resize").style(
                Style::default().fg(self.ui.theme.primary).bg(self.ui.theme.bg)
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
        if now.duration_since(self.images.last_resize) > Duration::from_millis(300) {
            let old_h = self.images.image_cell_height;
            self.update_image_cell_size();
            if self.images.image_cell_height != old_h && !self.images.image_cache.is_empty() {
                super::image::rebuild_image_protocols(self);
            }
            self.images.last_resize = now;
        }

        super::render::render_title_bar(self, f, title_area);
        super::render::render_sidebar(self, f, sidebar_area);
        super::render::render_animations(self, f, anim_area);
        self.ui.input_area = input_area;
        super::render::render_input(self, f, input_area);
        super::render::render_status_bar(self, f, status_area);
        super::render::render_messages(self, f, messages_area);

        if self.input.show_help {
            super::render::render_help_popup(self, f, area);
        }
    }
}

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
