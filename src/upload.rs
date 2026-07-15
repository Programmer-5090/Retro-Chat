use std::collections::HashMap;
use std::path::PathBuf;

use axum::{
    Router,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
    Json,
};
use serde::Serialize;
use tokio::fs;

use sqlx::Row;

use image::{ImageFormat, imageops::FilterType};

#[derive(Clone)]
pub struct UploadState {
    pub pool: sqlx::Pool<sqlx::Postgres>,
    pub redis_client: redis::Client,
}

#[derive(Serialize)]
pub struct UploadResponse {
    pub url: String,
    pub thumb_url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize)]
pub struct AudioUploadResponse {
    pub url: String,
    pub duration_ms: u32,
}

fn uploads_dir() -> PathBuf {
    PathBuf::from("uploads")
}

async fn verify_token(redis_client: &redis::Client, token: &str) -> Option<String> {
    let mut conn = redis_client.get_connection().ok()?;
    let username: String = redis::cmd("GET")
        .arg(format!("session:{}", token))
        .query(&mut conn)
        .ok()?;
    if username.is_empty() { None } else { Some(username) }
}

fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

fn audio_mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/aac" => "aac",
        "audio/mp4" | "audio/m4a" => "m4a",
        _ => "bin",
    }
}

fn is_audio_mime(mime: &str) -> bool {
    mime.starts_with("audio/")
}

fn audio_duration_ms(bytes: &[u8]) -> u32 {
    use std::io::Cursor;
    use rodio::Source;
    let cursor = Cursor::new(bytes.to_vec());
    match rodio::Decoder::new(cursor) {
        Ok(decoder) => {
            let total_frames = decoder.total_duration().map(|d| d.as_millis() as u32);
            total_frames.unwrap_or(0)
        }
        Err(_) => 0,
    }
}

async fn handle_upload(
    State(state): State<UploadState>,
    Query(params): Query<HashMap<String, String>>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, (StatusCode, String)> {
    let token = params
        .get("token")
        .ok_or((StatusCode::UNAUTHORIZED, "missing token".to_string()))?;
    let _username = verify_token(&state.redis_client, token)
        .await
        .ok_or((StatusCode::UNAUTHORIZED, "invalid token".to_string()))?;

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut original_name: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            original_name = field
                .file_name()
                .map(|s| s.to_string());
            let data = field
                .bytes()
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            if data.len() > 32 * 1024 * 1024 {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "file exceeds 32 MB limit".to_string(),
                ));
            }
            file_bytes = Some(data.to_vec());
        }
    }

    let bytes = file_bytes
        .ok_or((StatusCode::BAD_REQUEST, "no file field in multipart".to_string()))?;
    let img = image::load_from_memory(&bytes)
        .map_err(| e | (StatusCode::BAD_REQUEST, format!("Invalid image: {}", e)))?;
    let dims = (img.width(), img.height());

    let orig = original_name.unwrap_or_else(|| "upload".to_string());
    let thumb = img.resize(400, 400, FilterType::Lanczos3);

    let kind = infer::get(&bytes)
        .ok_or((StatusCode::BAD_REQUEST, "could not determine file type".to_string()))?;
    if !kind.mime_type().starts_with("image/") {
        return Err((
            StatusCode::BAD_REQUEST,
            "only image files are accepted".to_string(),
        ));
    }

    let mime = kind.mime_type();
    let ext = mime_to_ext(mime);
    let id = uuid::Uuid::new_v4().to_string();
    let filename = format!("{}.{}", id, ext);

    let dir = uploads_dir();
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let file_path = dir.join(&filename);
    fs::write(&file_path, &bytes)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let thumb_path = dir.join(format!("thumb_{}", &filename));
    let thumb_format = match mime {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => ImageFormat::Png,
    };
    let mut thumb_buf = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut thumb_buf, thumb_format)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("thumb encode: {}", e)))?;
    fs::write(&thumb_path, thumb_buf.into_inner())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let row = sqlx::query(
        "INSERT INTO attachments (filename, original_name, mime_type, file_path, thumb_path, uploader, width, height) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(&filename)
    .bind(&orig)
    .bind(mime)
    .bind(file_path.to_string_lossy().to_string())
    .bind(thumb_path.to_string_lossy().to_string())
    .bind(&_username)
    .bind(dims.0 as i32)
    .bind(dims.1 as i32)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let attachment_id: i32 = row.get("id");

    Ok(Json(UploadResponse {
        url: format!("/attachments/{}", attachment_id),
        thumb_url: format!("/attachments/{}/thumb", attachment_id),
        width: dims.0,
        height: dims.1,
    }))
}

async fn handle_audio_upload(
    State(state): State<UploadState>,
    Query(params): Query<HashMap<String, String>>,
    mut multipart: Multipart,
) -> Result<Json<AudioUploadResponse>, (StatusCode, String)> {
    let token = params
        .get("token")
        .ok_or((StatusCode::UNAUTHORIZED, "missing token".to_string()))?;
    let _username = verify_token(&state.redis_client, token)
        .await
        .ok_or((StatusCode::UNAUTHORIZED, "invalid token".to_string()))?;

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut original_name: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            original_name = field.file_name().map(|s| s.to_string());
            let data = field
                .bytes()
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            if data.len() > 50 * 1024 * 1024 {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "file exceeds 50 MB limit".to_string(),
                ));
            }
            file_bytes = Some(data.to_vec());
        }
    }

    let bytes = file_bytes
        .ok_or((StatusCode::BAD_REQUEST, "no file field in multipart".to_string()))?;

    let kind = infer::get(&bytes)
        .ok_or((StatusCode::BAD_REQUEST, "could not determine file type".to_string()))?;
    let mime = kind.mime_type();
    
    if !is_audio_mime(mime) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("only audio files are accepted, got: {}", kind.mime_type()),
        ));
    }

    
    let ext = audio_mime_to_ext(mime);
    let id = uuid::Uuid::new_v4().to_string();
    let filename = format!("{}.{}", id, ext);

    let dir = uploads_dir();
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let file_path = dir.join(&filename);
    fs::write(&file_path, &bytes)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let duration_ms = audio_duration_ms(&bytes);

    let orig = original_name.unwrap_or_else(|| "upload".to_string());

    let row = sqlx::query(
        "INSERT INTO attachments (filename, original_name, mime_type, file_path, thumb_path, uploader, width, height) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(&filename)
    .bind(&orig)
    .bind(mime)
    .bind(file_path.to_string_lossy().to_string())
    .bind("")
    .bind(&_username)
    .bind(0)
    .bind(0)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let attachment_id: i32 = row.get("id");

    Ok(Json(AudioUploadResponse {
        url: format!("/attachments/{}", attachment_id),
        duration_ms,
    }))
}

async fn get_attachment(
    State(state): State<UploadState>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let row = sqlx::query("SELECT file_path, mime_type FROM attachments WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "attachment not found".to_string()))?;

    let file_path: String = row.get("file_path");
    let mime_type: String = row.get("mime_type");

    let data = fs::read(&file_path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, mime_type.parse().unwrap());
    Ok((headers, data))
}

async fn get_attachment_thumb(
    State(state): State<UploadState>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let row = sqlx::query("SELECT thumb_path, file_path, mime_type FROM attachments WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "attachment not found".to_string()))?;

    let thumb_path: String = row.get("thumb_path");
    let file_path: String = row.get("file_path");
    let mime_type: String = row.get("mime_type");

    let path = if PathBuf::from(&thumb_path).exists() {
        thumb_path
    } else {
        file_path
    };

    let data = fs::read(&path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, mime_type.parse().unwrap());
    Ok((headers, data))
}

pub fn router(state: UploadState) -> Router {
    Router::new()
        .route("/upload", post(handle_upload))
        .route("/upload/audio", post(handle_audio_upload))
        .route("/attachments/{id}", get(get_attachment))
        .route("/attachments/{id}/thumb", get(get_attachment_thumb))
        .with_state(state)
}
