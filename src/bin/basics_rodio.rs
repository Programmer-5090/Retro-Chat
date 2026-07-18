use rodio::source::SineWave;
use rodio::Source;
use std::error::Error;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    let mut sink = rodio::DeviceSinkBuilder::open_default_sink()?;
    sink.log_on_drop(false);
    let mixer = sink.mixer();

    // 440 Hz sine for 2 seconds
    let wave1 = SineWave::new(440.0)
        .amplify(0.3)
        .take_duration(Duration::from_secs(2));
    mixer.add(wave1);
    println!("Playing 440 Hz tone...");
    thread::sleep(Duration::from_millis(2500));

    // 880 Hz sine for 2 seconds
    let wave2 = SineWave::new(880.0)
        .amplify(0.3)
        .take_duration(Duration::from_secs(2));
    mixer.add(wave2);
    println!("Playing 880 Hz tone...");
    thread::sleep(Duration::from_millis(2500));

    println!("Done — if you heard two tones, rodio output is working.");
    Ok(())
}
