mod audio;
mod config;
mod detector;
mod launcher;

use std::path::Path;
use std::thread;
use std::time::Duration;

use config::{Command, Config};
use detector::Detector;
use sherpa_onnx::Wave;

fn main() {
    let command = match Config::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error}\n\n{}", config::USAGE);
            std::process::exit(2);
        }
    };

    let result = match command {
        Command::Help => {
            println!("{}", config::USAGE);
            Ok(())
        }
        Command::Version => {
            println!("opencode-voice {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::ListDevices => audio::list_devices(),
        Command::Run(config) => run(config),
    };

    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run(config: Config) -> Result<(), String> {
    if let Some(wav) = &config.wav {
        return run_wav(&config, wav);
    }

    eprintln!(
        "loading wake-word model from {}",
        config.model_dir.display()
    );
    let mut detector = Detector::new(&config.model_dir, config.threshold)?;
    let mut capture = audio::Capture::open(config.device.as_deref())?;

    eprintln!(
        "ready | device: {} | input: {} Hz | threshold: {:.2} | listening for \"Hey, OpenCode\"",
        capture.device_name(),
        capture.sample_rate(),
        config.threshold
    );

    let mut buffer = vec![0.0f32; 8_192];

    loop {
        let count = capture.read(&mut buffer);
        if count == 0 {
            thread::sleep(Duration::from_millis(10));
            continue;
        }

        if let Some(keyword) = detector.accept(capture.sample_rate(), &buffer[..count]) {
            eprintln!("wake phrase detected: {keyword}");

            if config.dry_run {
                eprintln!("dry run: not launching Konsole");
            } else if let Err(error) = launcher::launch() {
                eprintln!("{error}");
            } else {
                eprintln!("launched Konsole with OpenCode");
            }

            detector.reset();
            drain(&mut capture, &mut buffer);

            if !config.cooldown.is_zero() {
                thread::sleep(config.cooldown);
                drain(&mut capture, &mut buffer);
            }
        }
    }
}

fn run_wav(config: &Config, path: &Path) -> Result<(), String> {
    let path = path
        .to_str()
        .ok_or_else(|| format!("WAV path is not valid UTF-8: {}", path.display()))?;
    let wave = Wave::read(path).ok_or_else(|| format!("could not read WAV file: {path}"))?;
    let mut detector = Detector::new(&config.model_dir, config.threshold)?;

    match detector.accept_wav(wave.sample_rate(), wave.samples()) {
        Some(keyword) => {
            println!("detected: {keyword}");
            Ok(())
        }
        None => {
            println!("no keyword detected");
            Ok(())
        }
    }
}

fn drain(capture: &mut audio::Capture, buffer: &mut [f32]) {
    while capture.read(buffer) > 0 {}
}
