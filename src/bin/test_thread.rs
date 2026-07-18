use rodio::Source;
use rodio::source::SineWave;
use std::error::Error;
use std::time::Duration;

fn main() -> Result<(), Box<dyn Error>> {
    println!("Opening sink on main thread (should work)...");
    let mut sink1 = rodio::DeviceSinkBuilder::open_default_sink()?;
    sink1.log_on_drop(false);
    let wave1 = SineWave::new(440.0).amplify(0.3).take_duration(Duration::from_secs(2));
    sink1.mixer().add(wave1);
    println!("Playing 440Hz on main thread...");
    std::thread::sleep(Duration::from_secs(3));
    drop(sink1);
    println!("Main thread done.");

    std::thread::sleep(Duration::from_secs(1));

    println!("Opening sink on spawn_blocking thread...");
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        tokio::task::spawn_blocking(|| {
            println!("[blocking] thread id: {:?}", std::thread::current().id());
            let mut sink2 = rodio::DeviceSinkBuilder::open_default_sink()
                .expect("failed to open sink");
            sink2.log_on_drop(false);
            let wave2 = SineWave::new(880.0).amplify(0.3).take_duration(Duration::from_secs(2));
            sink2.mixer().add(wave2);
            println!("[blocking] Playing 880Hz from spawn_blocking...");
            std::thread::sleep(Duration::from_secs(3));
            drop(sink2);
            println!("[blocking] Done.");
        }).await.unwrap();
    });

    println!("All done.");
    Ok(())
}
