mod audio;
mod config;
mod detector;
mod launcher;
mod open_code;

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use config::{Command, Config};
use detector::{Detector, VoiceCommand};
use launcher::LaunchMode;
use sherpa_onnx::Wave;

/// How long to wait after "Hey, OpenCode" for an optional "continue" or
/// "stop". Keeping the audio chunks small means the command word is not lost
/// when the base phrase fires first.
const COMMAND_GRACE: Duration = Duration::from_millis(1_500);
const STOP_COOLDOWN: Duration = Duration::from_secs(1);
const AUDIO_CHUNK: usize = 1_600;

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
        "ready | device: {} | input: {} Hz | threshold: {:.2} | listening for \"Hey, OpenCode\" (+ continue/stop)",
        capture.device_name(),
        capture.sample_rate(),
        config.threshold
    );

    let mut buffer = vec![0.0f32; AUDIO_CHUNK];
    let mut pending: Option<Instant> = None;

    loop {
        let count = capture.read(&mut buffer);

        if count > 0 {
            if let Some(detection) = detector.accept(capture.sample_rate(), &buffer[..count]) {
                match detection.command {
                    VoiceCommand::Launch => {
                        eprintln!("wake phrase detected: {}", detection.keyword);
                        pending = Some(Instant::now() + COMMAND_GRACE);
                    }
                    VoiceCommand::Continue | VoiceCommand::Stop => {
                        if pending.take().is_some() {
                            eprintln!(
                                "command detected: {} ({})",
                                detection.keyword,
                                command_name(detection.command)
                            );
                            execute_and_cooldown(
                                detection.command,
                                &config,
                                &mut detector,
                                &mut capture,
                                &mut buffer,
                            );
                            continue;
                        }
                    }
                }
            }
        }

        if pending.is_some_and(|deadline| Instant::now() >= deadline) {
            pending = None;
            eprintln!("no follow-up command; launching OpenCode");
            execute_and_cooldown(
                VoiceCommand::Launch,
                &config,
                &mut detector,
                &mut capture,
                &mut buffer,
            );
            continue;
        }

        if count == 0 {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn execute_and_cooldown(
    command: VoiceCommand,
    config: &Config,
    detector: &mut Detector,
    capture: &mut audio::Capture,
    buffer: &mut [f32],
) {
    let cooldown = execute(command, config);

    detector.reset();
    drain(capture, buffer);

    if !cooldown.is_zero() {
        thread::sleep(cooldown);
        drain(capture, buffer);
    }
}

fn execute(command: VoiceCommand, config: &Config) -> Duration {
    match command {
        VoiceCommand::Launch => {
            launch(LaunchMode::New, config);
            config.cooldown
        }
        VoiceCommand::Continue => {
            launch(LaunchMode::Continue, config);
            config.cooldown
        }
        VoiceCommand::Stop => {
            stop(config);
            STOP_COOLDOWN
        }
    }
}

fn launch(mode: LaunchMode, config: &Config) {
    let action = match mode {
        LaunchMode::New => "launch OpenCode",
        LaunchMode::Continue => "launch OpenCode with --continue",
    };

    if config.dry_run {
        eprintln!("dry run: would {action}");
        return;
    }

    match launcher::launch(mode) {
        Ok(()) => eprintln!("launched Konsole: {action}"),
        Err(error) => eprintln!("{error}"),
    }
}

fn stop(config: &Config) {
    if config.dry_run {
        eprintln!("dry run: would interrupt active OpenCode sessions");
        return;
    }

    match open_code::stop_active_sessions() {
        Ok(report) if report.active == 0 => {
            eprintln!("stop: no active OpenCode response");
        }
        Ok(report) => {
            eprintln!(
                "stop: interrupted {}/{} active session(s)",
                report.interrupted, report.active
            );
            for error in report.errors {
                eprintln!("stop: {error}");
            }
        }
        Err(error) => eprintln!("stop failed: {error}"),
    }
}

fn run_wav(config: &Config, path: &Path) -> Result<(), String> {
    let path = path
        .to_str()
        .ok_or_else(|| format!("WAV path is not valid UTF-8: {}", path.display()))?;
    let wave = Wave::read(path).ok_or_else(|| format!("could not read WAV file: {path}"))?;
    let sample_rate = wave.sample_rate();
    if sample_rate <= 0 {
        return Err("WAV file has an invalid sample rate".to_string());
    }

    let mut detector = Detector::new(&config.model_dir, config.threshold)?;
    let mut samples = wave.samples().to_vec();
    let padding = vec![0.0f32; sample_rate as usize];
    samples.extend_from_slice(&padding);

    let mut elapsed = Duration::ZERO;
    let mut pending: Option<(String, Duration)> = None;

    for chunk in samples.chunks(AUDIO_CHUNK) {
        if let Some(detection) = detector.accept(sample_rate as u32, chunk) {
            match detection.command {
                VoiceCommand::Launch => {
                    pending = Some((detection.keyword, elapsed + COMMAND_GRACE));
                }
                VoiceCommand::Continue | VoiceCommand::Stop => {
                    if pending.take().is_some() {
                        println!(
                            "detected: {} ({})",
                            detection.keyword,
                            command_name(detection.command)
                        );
                        return Ok(());
                    }
                }
            }
        }

        elapsed += Duration::from_secs_f64(chunk.len() as f64 / sample_rate as f64);

        if let Some((keyword, deadline)) = &pending {
            if elapsed >= *deadline {
                println!("detected: {keyword} (launch)");
                return Ok(());
            }
        }
    }

    match pending {
        Some((keyword, _)) => println!("detected: {keyword} (launch)"),
        None => println!("no keyword detected"),
    }

    Ok(())
}

fn command_name(command: VoiceCommand) -> &'static str {
    match command {
        VoiceCommand::Launch => "launch",
        VoiceCommand::Continue => "continue",
        VoiceCommand::Stop => "stop",
    }
}

fn drain(capture: &mut audio::Capture, buffer: &mut [f32]) {
    while capture.read(buffer) > 0 {}
}
