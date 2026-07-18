use tokio::io::AsyncWriteExt;

use super::app::App;
use crate::message::dm_display_name;
use super::format::make_system_msg;

pub(crate) const HELP_COMMANDS: [(&'static str, &'static str, &'static str); 10] = [
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

async fn cmd_join(app: &mut App, room_arg: &str) {
    let room = room_arg.trim();
    if room.is_empty() || room.len() > 32 || room.contains(char::is_whitespace) {
        app.ingest_msg(
            make_system_msg("Usage: /join <room>  (1\u{2013}32 non-whitespace chars)"),
            true
        );
    } else {
        let resolved = if !room.starts_with("__dm__") {
            let mut dm_users = vec![app.username.clone(), room.to_string()];
            dm_users.sort();
            let dm_room = format!("__dm__{}", dm_users.join("_"));
            if app.rooms.iter().any(|r| r == &dm_room) {
                dm_room
            } else {
                room.to_string()
            }
        } else {
            room.to_string()
        };
        if !app.rooms.iter().any(|r| r == &resolved) {
            app.rooms.push(resolved.clone());
        }
        app.current_room = resolved.clone();
        app.mark_all_read(&resolved).await;
        app.scroll_offset = 0;
        app.ingest_msg(
            make_system_msg(&format!("Joined room: {}", dm_display_name(&resolved, &app.username))),
            true
        );
        let msg = format!("/join {}\n", resolved);
        let _ = app.writer.lock().await.write_all(msg.as_bytes()).await;
    }
}

async fn cmd_leave(app: &mut App) {
    if app.rooms.len() > 1 {
        let left = app.current_room.clone();
        app.rooms.retain(|r| r != &app.current_room);
        app.current_room = app.rooms[0].clone();
        let cur = app.current_room.clone();
        app.clear_room_read_state(&cur);
        app.ingest_msg(
            make_system_msg(&format!("Left room: {}", dm_display_name(&left, &app.username))),
            true
        );
        let _ = app.writer.lock().await.write_all(b"/leave\n").await;
    } else {
        app.ingest_msg(make_system_msg("Cannot leave the last room."), true);
    }
}

async fn cmd_quit(app: &mut App) {
    // Should stop any in-flight playback when called
    super::audio::stop_playback(app);
    app.should_quit = true;
    let _ = app.writer.lock().await.shutdown().await;
}

fn cmd_clear(app: &mut App) {
    app.messages.retain(|(msg, _)| msg.room != app.current_room && !msg.room.is_empty());
    app.scroll_offset = 0;
}

fn cmd_help(app: &mut App) {
    app.input.show_help = true;
    app.input.help_state.select(Some(0));
}

async fn cmd_rooms(app: &mut App) {
    let _ = app.writer.lock().await.write_all(b"/rooms\n").await;
}

async fn cmd_dm(app: &mut App, user_arg: Option<&str>, msg_arg: Option<&str>) {
    let user = user_arg.unwrap_or("").trim();
    let dm_msg = msg_arg.unwrap_or("").trim();
    if user.is_empty() || dm_msg.is_empty() {
        app.ingest_msg(make_system_msg("Usage: /dm <user> <message>"), true);
    } else {
        let mut users = vec![app.username.clone(), user.to_string()];
        users.sort();
        let dm_room = format!("__dm__{}", users.join("_"));
        if !app.rooms.iter().any(|r| r == &dm_room) {
            app.rooms.push(dm_room.clone());
        }
        app.current_room = dm_room.clone();
        app.mark_all_read(&dm_room).await;
        app.scroll_offset = 0;
        let wire = format!("/msg {} {}\n", user, dm_msg);
        let _ = app.writer.lock().await.write_all(wire.as_bytes()).await;
    }
}

fn cmd_record(app: &mut App) {
    if app.audio.is_recording {
        super::audio::stop_recording(app);
    } else {
        super::audio::start_recording(app);
    }
}

fn cmd_audio(app: &mut App, parts: Vec<&str>) {
    let path = parts.get(1).copied().unwrap_or("").trim();
    if path.is_empty() {
        app.ingest_msg(make_system_msg("Usage: /audio <file_path>"), true);
    } else {
        let path = path.to_string();
        let token = app.token.clone();
        let writer = app.writer.clone();
        let err_tx = app.server_tx.clone();
        app.ingest_msg(make_system_msg("Uploading audio..."), true);
        tokio::spawn(async move {
            match super::audio::do_audio_upload(&path, &token).await {
                Ok(resp) => {
                    let wire = format!("/audio {} {}\n", resp.url, resp.duration_ms);
                    let _ = writer.lock().await.write_all(wire.as_bytes()).await;
                }
                Err(e) => {
                    let _ = err_tx.send(format!("Audio upload failed: {}", e));
                }
            }
        });
    }
}

async fn cmd_image(app: &mut App, parts: Vec<&str>) {
    let path = parts.get(1).copied().unwrap_or("").trim();
    if path.is_empty() {
        app.ingest_msg(make_system_msg("Usage: /image <file_path>"), true);
    } else {
        let path = path.to_string();
        super::image::upload_and_send_image(app, path).await;
    }
}

pub(crate) async fn send_or_command(app: &mut App, text: String) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }

    if text.starts_with('/') {
        let parts: Vec<&str> = text.splitn(3, ' ').collect();
        let cmd = parts[0];

        match cmd {
            "/help" => cmd_help(app),
            "/image" => cmd_image(app, parts).await,
            "/audio" => cmd_audio(app, parts),
            "/record" => cmd_record(app),
            "/clear" => cmd_clear(app),
            "/quit" => cmd_quit(app).await,
            "/join" => cmd_join(app, parts.get(1).copied().unwrap_or("").trim()).await,
            "/leave" => cmd_leave(app).await,
            "/rooms" => cmd_rooms(app).await,
            "/dm" => cmd_dm(app, parts.get(1).copied(), parts.get(2).copied()).await,
            _ => {
                app.ingest_msg(make_system_msg(&format!("Unknown command: {}", text)), true);
            }
        }
    } else {
        let cur = app.current_room.clone();
        app.mark_all_read(&cur).await;
        let wire = format!("{}\n", text);
        let _ = app.writer.lock().await.write_all(wire.as_bytes()).await;
    }
}
