use cursive::{
    Cursive,
    align::HAlign,
    event::Key,
    theme::{ BaseColor, Color },
    traits::*,
    views::{ Dialog, DummyView, EditView, LinearLayout, Panel, ScrollView, TextView },
};
use chrono::Local;
use std::sync::Arc;
use tokio::{
    io::{ AsyncBufReadExt, AsyncWriteExt, BufReader },
    sync::Mutex,
};

use crate::client_helpers::{ ClientStream, send_message, create_retro_theme };
use crate::{ ChatMessage, MessageType };

pub async fn run_chat_ui(
    username: String,
    reader: BufReader<tokio::io::ReadHalf<ClientStream>>,
    writer: tokio::io::WriteHalf<ClientStream>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut siv = cursive::default();
    siv.set_theme(create_retro_theme());

    let header = TextView::new(
        format!(r#"╔═ RETRO CHAT ═╗ User: {} ╔═ {} ═╗"#, username, Local::now().format("%H:%M:%S"))
    )
        .style(Color::Light(BaseColor::Green))
        .h_align(HAlign::Center);

    let messages = TextView::new("").with_name("messages").min_height(20).scrollable();

    let messages = ScrollView::new(messages)
        .scroll_strategy(cursive::view::ScrollStrategy::StickToBottom)
        .min_width(60)
        .full_width();

    let input = EditView::new()
        .on_submit(move |s, text| send_message(s, text.to_string()))
        .with_name("input")
        .min_width(50)
        .max_height(3)
        .full_width();

    let help_text = TextView::new("ESC:quit | Enter:send | Commands: /help, /clear, /quit")
        .style(Color::Dark(BaseColor::White));

    let layout = LinearLayout::vertical()
        .child(Panel::new(header))
        .child(
            Dialog::around(messages).title("Messages").title_position(HAlign::Center).full_width()
        )
        .child(Dialog::around(input).title("Message").title_position(HAlign::Center).full_width())
        .child(Panel::new(help_text).full_width());

    let centered_layout = LinearLayout::horizontal()
        .child(DummyView.full_width())
        .child(layout)
        .child(DummyView.full_width());

    siv.add_fullscreen_layer(centered_layout);
    siv.add_global_callback(Key::Esc, |s| s.quit());
    siv.add_global_callback('/', |s| {
        s.call_on_name("input", |view: &mut EditView| {
            view.set_content("/");
        });
    });

    let writer = Arc::new(Mutex::new(writer));
    let writer_clone = Arc::clone(&writer);
    siv.set_user_data(writer);

    let mut lines = reader.lines();
    let sink = siv.cb_sink().clone();

    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(msg) = serde_json::from_str::<ChatMessage>(&line) {
                let formatted_msg = match msg.message_type {
                    MessageType::UserMessage => {
                        format!("┌─[{}]\n└─ {} ▶ {}\n", msg.timestamp, msg.username, msg.content)
                    }
                    MessageType::SystemNotification => {
                        format!("\n[{} {}]\n", msg.username, msg.content)
                    }
                };

                if
                    sink
                        .send(
                            Box::new(move |siv: &mut Cursive| {
                                siv.call_on_name("messages", |view: &mut TextView| {
                                    view.append(formatted_msg);
                                });
                            })
                        )
                        .is_err()
                {
                    break;
                }
            }
        }
    });

    siv.run();
    let _ = writer_clone.lock().await.shutdown().await;
    Ok(())
}
