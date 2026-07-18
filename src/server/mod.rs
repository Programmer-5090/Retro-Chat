mod auth;
mod rooms;
mod commands;
mod messages;
mod connection;

pub use connection::handle_connection;

pub(super) async fn send_notice(writer: &mut (impl tokio::io::AsyncWrite + Unpin), text: &str) {
    let notice = crate::build_notice(text);
    let _ = tokio::io::AsyncWriteExt::write_all(writer, notice.as_bytes()).await;
}
