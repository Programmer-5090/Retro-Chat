use std::sync::Arc;
use std::sync::atomic::{ AtomicBool, Ordering };
use std::time::Instant;

use cpal::traits::{ HostTrait, DeviceTrait, StreamTrait };
use tokio::io::AsyncWriteExt;

use super::app::App;
use super::anims::{ SpectrumState, TAP_BUFFER_CAPACITY };
use crate::message::MessageType;
use super::format::make_system_msg;

#[derive(serde::Deserialize)]
pub(crate) struct AudioUploadResponse {
    pub url: String,
    pub duration_ms: u32,
}

fn decode_mono_samples(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    use rodio::Source;
    let decoder = rodio::Decoder::new(std::io::Cursor::new(bytes.to_vec())).ok()?;
    let channels = decoder.channels().get() as usize;
    let sample_rate = decoder.sample_rate().get();
    let samples: Vec<f32> = decoder.into_iter().collect();
    if samples.is_empty() || channels == 0 {
        return None;
    }
    let mono: Vec<f32> = if channels > 1 {
        samples
            .chunks(channels)
            .map(|c| c.iter().sum::<f32>() / (channels as f32))
            .collect()
    } else {
        samples
    };
    Some((mono, sample_rate))
}

pub(crate) fn toggle_play_audio(app: &mut App) {
    if let Some(ref _playing_id) = app.audio.playing_audio.clone() {
        if let Some(flag) = app.audio.spectrum_stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        app.audio.live_spectrum.clear();
        app.audio.playing_audio = None;
        app.ingest_msg(make_system_msg("Stopped playback."), true);
        return;
    }

    let audio_msg = app
        .messages_for_room(&app.current_room)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find(
            |(msg, _)|
                msg.message_type == MessageType::AudioMessage && !msg.audio_note_url.is_empty()
        )
        .map(|(msg, _)| msg.clone());

    if let Some(msg) = audio_msg {
        let msg_id = msg.id.clone();
        let audio_url = msg.audio_note_url.clone();
        let upload_base = std::env
            ::var("UPLOAD_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8083".to_string());
        let full_url = format!("{}{}", upload_base, audio_url);
        let err_tx = app.server_tx.clone();
        let playing_id = msg_id.clone();

        app.audio.playing_audio = Some(playing_id.clone());
        app.ingest_msg(make_system_msg("Playing audio..."), true);

        let outer_stop_flag = Arc::new(AtomicBool::new(false));
        app.audio.spectrum_stop = Some(outer_stop_flag.clone());
        let outer_spectrum_tx = app.audio.spectrum_tx.clone();
        let outer_ticker_id = playing_id.clone();

        tokio::spawn(async move {
            match reqwest::get(&full_url).await {
                Ok(resp) => {
                    match resp.bytes().await {
                        Ok(bytes) => {
                            let audio_bytes = bytes.to_vec();

                            let ticker_id = outer_ticker_id;
                            let spectrum_tx = outer_spectrum_tx;
                            let stop_flag = outer_stop_flag;

                            if let Some((mono_samples, sample_rate)) = decode_mono_samples(&audio_bytes) {
                                tokio::spawn(async move {
                                    let mut spectrum = SpectrumState::default();
                                    let start = Instant::now();
                                    let total_samples = mono_samples.len();
                                    loop {
                                        if stop_flag.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        let elapsed = start.elapsed().as_secs_f32();
                                        let pos = ((elapsed * (sample_rate as f32)) as usize).min(total_samples);
                                        if pos >= total_samples {
                                            break;
                                        }
                                        let window_start = pos.saturating_sub(TAP_BUFFER_CAPACITY);
                                        let window = &mono_samples[window_start..pos];
                                        if window.len() >= 64 {
                                            spectrum.update(window, sample_rate);
                                            if spectrum_tx.send((ticker_id.clone(), spectrum.bins().to_vec())).is_err() {
                                                break;
                                            }
                                        }
                                        tokio::time::sleep(std::time::Duration::from_millis(33)).await;
                                    }
                                });
                            }

                            let err_tx_for_play = err_tx.clone();
                            let result = tokio::task::spawn_blocking(move || {
                                let cursor = std::io::Cursor::new(audio_bytes);
                                let mut handle = match
                                    rodio::DeviceSinkBuilder::open_default_sink()
                                {
                                    Ok(h) => h,
                                    Err(e) => {
                                        let _ = err_tx_for_play.send(
                                            format!("Audio device error: {}", e)
                                        );
                                        return;
                                    }
                                };
                                handle.log_on_drop(false);
                                let mixer = handle.mixer();

                                let player = match rodio::play(mixer, cursor) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        let _ = err_tx_for_play.send(
                                            format!("Audio play error: {}", e)
                                        );
                                        return;
                                    }
                                };
                                player.set_volume(0.5);

                                while !player.empty() {
                                    std::thread::sleep(std::time::Duration::from_millis(100));
                                }
                                drop(player);
                                drop(handle);
                                let _ = err_tx_for_play.send("__PLAYBACK_DONE__".to_string());
                            }).await;
                            if let Err(join_err) = &result {
                                let _ = err_tx.send(
                                    format!("Audio playback panicked: {}", join_err)
                                );
                            }
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

pub(crate) async fn do_audio_upload(
    path: &str,
    token: &str
) -> Result<AudioUploadResponse, String> {
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

pub(crate) fn start_recording(app: &mut App) {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            app.ingest_msg(make_system_msg("No input device found."), true);
            return;
        }
    };
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            app.ingest_msg(make_system_msg(&format!("Mic config error: {}", e)), true);
            return;
        }
    };

    let channels = config.channels();
    let sample_rate = config.sample_rate();
    let channels_nz = std::num::NonZeroU16
        ::new(channels)
        .unwrap_or(std::num::NonZeroU16::new(1).unwrap());
    let sample_rate_nz = std::num::NonZeroU32
        ::new(sample_rate)
        .unwrap_or(std::num::NonZeroU32::new(44100).unwrap());

    let samples_buf = Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
    let samples_for_cb = samples_buf.clone();

    let err_tx = app.server_tx.clone();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = samples_for_cb.lock() {
                        buf.extend_from_slice(data);
                    }
                },
                move |err| {
                    let _ = err_tx.send(format!("Mic error: {}", err));
                },
                None
            )
        }
        cpal::SampleFormat::I16 => {
            let sb = samples_for_cb.clone();
            let et = err_tx.clone();
            device.build_input_stream(
                &(cpal::StreamConfig {
                    channels,
                    sample_rate,
                    buffer_size: cpal::BufferSize::Default,
                }),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = sb.lock() {
                        for &s in data {
                            buf.push((s as f32) / (i16::MAX as f32));
                        }
                    }
                },
                move |err| {
                    let _ = et.send(format!("Mic error: {}", err));
                },
                None
            )
        }
        fmt => {
            app.ingest_msg(make_system_msg(&format!("Unsupported mic format: {:?}", fmt)), true);
            return;
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            app.ingest_msg(make_system_msg(&format!("Mic open error: {}", e)), true);
            return;
        }
    };

    if let Err(e) = stream.play() {
        app.ingest_msg(make_system_msg(&format!("Mic start error: {}", e)), true);
        return;
    }

    app.audio.is_recording = true;
    app.audio.record_start = Some(Instant::now());
    app.audio.record_stream = Some(stream);
    app.audio.record_samples = Some(samples_buf);
    app.audio.record_channels = Some(channels_nz);
    app.audio.record_sample_rate = Some(sample_rate_nz);

    app.ingest_msg(make_system_msg("Recording audio... (type /record again to stop)"), true);
}

pub(crate) fn stop_recording(app: &mut App) {
    app.audio.is_recording = false;
    let elapsed = app.audio.record_start.map(|s| s.elapsed().as_millis() as u32).unwrap_or(0);
    app.audio.record_start = None;

    app.audio.record_stream.take();

    let samples = app.audio.record_samples
        .take()
        .map(|arc| {
            match Arc::try_unwrap(arc) {
                Ok(mutex) => mutex.into_inner().unwrap(),
                Err(arc) => arc.lock().unwrap().clone(),
            }
        })
        .unwrap_or_default();
    let channels = app.audio.record_channels.take();
    let sample_rate = app.audio.record_sample_rate.take();

    if samples.is_empty() || channels.is_none() || sample_rate.is_none() {
        app.ingest_msg(make_system_msg("No audio captured."), true);
        return;
    }

    let channels = channels.unwrap();
    let sample_rate = sample_rate.unwrap();

    let trim = ((sample_rate.get() as usize) * (channels.get() as usize)) / 10;
    let samples = if samples.len() > trim { samples[trim..].to_vec() } else { samples };

    app.ingest_msg(
        make_system_msg(
            &format!(
                "Recording stopped ({:.1}s). Encoding and uploading...",
                (elapsed as f64) / 1000.0
            )
        ),
        true
    );

    let token = app.token.clone();
    let writer = app.writer.clone();
    let err_tx = app.server_tx.clone();

    tokio::spawn(async move {
        let wav_path = std::env::temp_dir().join("retro_audio_record.wav");
        let spec = hound::WavSpec {
            channels: channels.get(),
            sample_rate: sample_rate.get(),
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav_writer = match hound::WavWriter::create(&wav_path, spec) {
            Ok(w) => w,
            Err(e) => {
                let _ = err_tx.send(format!("WAV create error: {}", e));
                return;
            }
        };
        for &s in &samples {
            let sample_i16 = (s.clamp(-1.0, 1.0) * (i16::MAX as f32)) as i16;
            if let Err(e) = wav_writer.write_sample(sample_i16) {
                let _ = err_tx.send(format!("WAV write error: {}", e));
                return;
            }
        }
        if let Err(e) = wav_writer.finalize() {
            let _ = err_tx.send(format!("WAV finalize error: {}", e));
            return;
        }
        match do_audio_upload(&wav_path.to_string_lossy(), &token).await {
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
