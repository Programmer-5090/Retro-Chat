use std::time::{ Duration, Instant };

use crossterm::{
    event::{ self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind },
    execute,
    terminal::{ EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode },
};
use ratatui::{
    Frame,
    layout::{ Alignment, Constraint, Layout },
    prelude::{ CrosstermBackend, Terminal },
    style::{ Color, Style },
    symbols::border,
    text::{ Line, Span },
    widgets::{ Block, Borders, Paragraph, Wrap },
};
use tokio::io::{ AsyncBufReadExt, AsyncWriteExt, BufReader };

use crate::ChatMessage;
use crate::client_helpers::ClientStream;

const LOGO: &str =
    r#"
        ██████╗ ██╗   ██╗████████╗███████╗ ██████╗██╗  ██╗ █████╗ ████████╗    
        ██╔══██╗╚██╗ ██╔╝╚══██╔══╝██╔════╝██╔════╝██║  ██║██╔══██╗╚══██╔══╝    
        ██████╔╝ ╚████╔╝    ██║   █████╗  ██║     ███████║███████║   ██║       
        ██╔══██╗  ╚██╔╝     ██║   ██╔══╝  ██║     ██╔══██║██╔══██║   ██║       
        ██████╔╝   ██║      ██║   ███████╗╚██████╗██║  ██║██║  ██║   ██║       
        ╚═════╝    ╚═╝      ╚═╝   ╚══════╝ ╚═════╝╚═╝  ╚═╝╚═╝  ╚═╝   ╚═╝                                                                             
    "#;

struct CleanGuard;

impl Drop for CleanGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

/// Shows the ByteChat splash screen with a typewriter reveal, then collects
/// the account password (register or login, per the server's first message)
/// inside the same terminal UI. Returns the reader/writer once the server
/// confirms auth with a session token, ready to give to run_chat_ui.
pub async fn run_login_ui(
    mut reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
    mut writer: tokio::io::WriteHalf<ClientStream>
) -> Result<
    (BufReader<tokio::io::ReadHalf<ClientStream>>, tokio::io::WriteHalf<ClientStream>, String),
    Box<dyn std::error::Error>
> {
    enable_raw_mode()?;
    let _guard = CleanGuard;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    run_splash(&mut terminal)?;

    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let mut prompt_msg: ChatMessage = serde_json::from_str(line.trim())?;

    let mut password = String::new();
    let mut status: Option<String> = None;

    loop {
        terminal.draw(|f|
            draw_password_screen(f, &prompt_msg.content, &password, status.as_deref())
        )?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char(c) => {
                        password.push(c);
                    }
                    KeyCode::Backspace => {
                        password.pop();
                    }
                    KeyCode::Enter => {
                        if password.is_empty() {
                            continue;
                        }
                        let cmd = if prompt_msg.content.contains("Register") {
                            format!("/register {}\n", password)
                        } else {
                            format!("/login {}\n", password)
                        };
                        writer.write_all(cmd.as_bytes()).await?;

                        let resp = loop {
                            let mut resp_line = String::new();
                            let n = reader.read_line(&mut resp_line).await?;
                            if n == 0 {
                                break None;
                            }
                            match serde_json::from_str::<ChatMessage>(resp_line.trim()) {
                                Ok(m) if m.username == "Server" => {
                                    break Some(m);
                                }
                                Ok(_) => {
                                    continue;
                                }
                                Err(_) => {
                                    break None;
                                }
                            }
                        };

                        match resp {
                            Some(resp) if resp.content.contains("Token:") => {
                                prompt_msg = resp;
                                break;
                            }
                            Some(resp) => {
                                password.clear();
                                status = Some(resp.content.clone());
                                prompt_msg = resp;
                            }
                            None => {
                                status = Some("Connection closed by server".to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;

    let token = prompt_msg.content
        .split("Token: ")
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_string();

    Ok((reader, writer, token))
}

/// Reveals the ByteChat logo a couple of characters at a time. Any keypress
/// skips straight to the full logo (and the short hold that follows).
fn run_splash(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>
) -> Result<(), Box<dyn std::error::Error>> {
    let lines: Vec<&str> = LOGO.lines().collect();
    let mut revealed_lines = 0usize;
    let mut revealed_cols = 0usize;

    while revealed_lines < lines.len() {
        terminal.draw(|f| draw_splash(f, &lines, revealed_lines, revealed_cols))?;

        if event::poll(Duration::from_millis(8))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    revealed_lines = lines.len();
                    break;
                }
            }
        }

        let cur_line_len = lines[revealed_lines].chars().count();
        revealed_cols += 2;
        if revealed_cols >= cur_line_len {
            revealed_cols = 0;
            revealed_lines += 1;
        }
    }

    terminal.draw(|f| draw_splash(f, &lines, lines.len(), 0))?;

    let hold_until = Instant::now() + Duration::from_millis(700);
    while Instant::now() < hold_until {
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    break;
                }
            }
        }
    }

    Ok(())
}

fn draw_splash(f: &mut Frame, lines: &[&str], revealed_lines: usize, revealed_cols: usize) {
    let area = f.area();
    let amber = Style::default().fg(Color::Rgb(255, 176, 0));

    let mut out_lines: Vec<Line> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if i < revealed_lines {
            out_lines.push(Line::from(Span::styled(l.to_string(), amber)));
        } else if i == revealed_lines {
            let partial: String = l.chars().take(revealed_cols).collect();
            out_lines.push(Line::from(Span::styled(partial, amber)));
            break;
        } else {
            break;
        }
    }
    while out_lines.len() < lines.len() {
        out_lines.push(Line::from(""));
    }
    out_lines.push(Line::from(""));
    if revealed_lines >= lines.len() {
        out_lines.push(
            Line::from(
                Span::styled(
                    "byte-sized chat for the terminal",
                    Style::default().fg(Color::DarkGray)
                )
            )
        );
    }

    let logo_width = (lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16).min(
        area.width
    );
    let block_height = (out_lines.len() as u16).min(area.height);

    let [_, vmid, _] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(block_height),
        Constraint::Min(0),
    ]).areas(area);
    let [_, hmid, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(logo_width),
        Constraint::Min(0),
    ]).areas(vmid);

    let p = Paragraph::new(out_lines).alignment(Alignment::Left);
    f.render_widget(p, hmid);
}

fn draw_password_screen(f: &mut Frame, prompt: &str, password: &str, status: Option<&str>) {
    let area = f.area();
    let amber = Style::default().fg(Color::Rgb(255, 176, 0));
    let masked: String = "*".repeat(password.chars().count());

    let box_w = 54u16.min(area.width.saturating_sub(4)).max(20);
    let box_h = 9u16.min(area.height.saturating_sub(2)).max(7);

    let [_, vmid, _] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(box_h),
        Constraint::Min(0),
    ]).areas(area);
    let [_, hmid, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(box_w),
        Constraint::Min(0),
    ]).areas(vmid);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(amber)
        .title(" ByteChat Login ");
    let inner = block.inner(hmid);
    f.render_widget(block, hmid);

    let mut lines = vec![
        Line::from(Span::styled(prompt.to_string(), Style::default().fg(Color::Cyan))),
        Line::from(""),
        Line::from(
            vec![
                Span::styled("Password: ", Style::default().fg(Color::DarkGray)),
                Span::styled(masked, amber),
                Span::styled("_", amber)
            ]
        )
    ];
    if let Some(s) = status {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(s.to_string(), Style::default().fg(Color::Red))));
    }

    let p = Paragraph::new(lines).wrap(Wrap { trim: true });
    f.render_widget(p, inner);
}
