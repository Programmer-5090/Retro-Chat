use rodio::Source;
use rodio::microphone::MicrophoneBuilder;
use std::error::Error;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("Opening default input device...");
    let mic = MicrophoneBuilder::new()
        .default_device()
        .map_err(|e| format!("No input device: {}", e))?
        .default_config()
        .map_err(|e| format!("Mic config error: {}", e))?
        .open_stream()
        .map_err(|e| format!("Mic open error: {}", e))?;

    let sample_rate = mic.sample_rate();
    let channels = mic.channels();
    println!(
        "Recording 3 seconds... ({}Hz, {} ch)",
        sample_rate.get(),
        channels.get()
    );

    let recording = mic
        .take_duration(Duration::from_secs(3))
        .record();

    let mut samples: Vec<f32> = Vec::new();
    for s in recording {
        samples.push(s);
    }
    println!("Captured {} raw samples", samples.len());

    let channels_n = channels.get() as usize;
    let usable_len = samples.len() - (samples.len() % channels_n);
    samples.truncate(usable_len);

    if samples.is_empty() {
        return Err("No audio captured".into());
    }

    let wav_path = std::env::temp_dir().join("retro_test_record.wav");
    let spec = hound::WavSpec {
        channels: channels.get(),
        sample_rate: sample_rate.get(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&wav_path, spec)?;
    for &s in &samples {
        let sample_i16 = (s.clamp(-1.0, 1.0) * (i16::MAX as f32)) as i16;
        writer.write_sample(sample_i16)?;
    }
    writer.finalize()?;
    println!("Saved WAV to {}", wav_path.display());

    println!("Playing back in 1 second...");
    thread::sleep(Duration::from_secs(1));

    let mut sink = rodio::DeviceSinkBuilder::open_default_sink()?;
    sink.log_on_drop(false);
    let mixer = sink.mixer();

    let file = std::fs::File::open(&wav_path)?;
    let player = rodio::play(mixer, std::io::BufReader::new(file))?;
    player.set_volume(0.5);

    println!("Playing recording...");
    thread::sleep(Duration::from_secs(5));

    println!("Done.");
    Ok(())
}
