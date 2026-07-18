use ratatui_image::Resize;

use super::app::App;
use tokio::io::AsyncWriteExt;

#[derive(serde::Deserialize)]
pub(crate) struct ImageUploadResponse {
    pub url: String,
    pub thumb_url: String,
    pub width: u32,
    pub height: u32,
}

pub(crate) fn spawn_image_fetch(app: &App, msg_id: String, thumb_url: String) {
    let tx = app.images.image_results_tx.clone();
    let picker = app.images.picker.clone();
    let upload_base = std::env
        ::var("UPLOAD_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8083".to_string());
    let url = format!("{}{}", upload_base, thumb_url);
    let cell_w = app.images.image_cell_width;
    let cell_h = app.images.image_cell_height;

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

pub(crate) fn rebuild_image_protocols(app: &mut App) {
    let old_cache = std::mem::take(&mut app.images.image_cache);
    for (id, _old_proto) in old_cache {
        if let Some(msg) = app.messages.iter().find(|(m, _)| m.id == id) {
            let msg = &msg.0;
            if !msg.thumb_url.is_empty() {
                app.images.inflight_images.remove(&id);
                spawn_image_fetch(app, id, msg.thumb_url.clone());
            }
        }
    }
}

pub(crate) async fn do_image_upload(
    path: &str,
    token: &str
) -> Result<ImageUploadResponse, String> {
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

    let form = reqwest::multipart::Form::new().part("file", part);

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

    serde_json
        ::from_str::<ImageUploadResponse>(&body)
        .map_err(|e| format!("invalid response: {}", e))
}

pub(crate) async fn upload_and_send_image(app: &mut App, path: String) {
    let token = app.token.clone();
    let writer = app.writer.clone();
    let err_tx = app.server_tx.clone();
    app.ingest_msg(super::format::make_system_msg("Uploading image..."), true);
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
