mod audio;
mod config;
mod detector;
mod gate;
mod launcher;
mod notify;
mod open_code;

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use config::{Command, Config};
use detector::{Detector, VoiceCommand};
use gate::{GateAction, WakeGate};
use launcher::LaunchMode;
use sherpa_onnx::Wave;

/// After the wake phrase, listen this long for an optional command. If nothing
/// is heard, open OpenCode normally.
const LISTEN_WINDOW: Duration = Duration::from_secs(5);
const WAKE_ECHO_GUARD: Duration = Duration::from_millis(300);
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
    let mut capture = audio::Capture::open(config.device.as_deref())?;
    let mut detector = Detector::new(&config.model_dir, config.threshold, capture.sample_rate())?;
    let mut gate = WakeGate::new(capture.sample_rate(), config.energy_gate);

    let gate_status = if config.energy_gate > 0.0 {
        format!("{:.3}", config.energy_gate)
    } else {
        "off".to_string()
    };
    eprintln!(
        "ready | device: {} | input: {} Hz | threshold: {:.2} | gate: {gate_status} | listening for \"Hey, OpenCode\"",
        capture.device_name(),
        capture.sample_rate(),
        config.threshold
    );

    let mut buffer = vec![0.0f32; AUDIO_CHUNK];
    let mut listening_until: Option<Instant> = None;
    let mut notice: Option<notify::ListeningNotice> = None;
    let mut ignore_open_until: Option<Instant> = None;

    loop {
        let count = capture.read(&mut buffer);

        if count > 0 {
            if listening_until.is_some() {
                if let Some(detection) =
                    detector.accept_command(capture.sample_rate(), &buffer[..count])
                {
                    if detection.command == VoiceCommand::Open
                        && ignore_open_until.is_some_and(|deadline| Instant::now() < deadline)
                    {
                        eprintln!("ignored OPEN from the wake phrase; still listening");
                        continue;
                    }

                    listening_until = None;
                    notice = None;
                    ignore_open_until = None;
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
            } else {
                match gate.filter(&buffer[..count]) {
                    GateAction::Skip => {}
                    GateAction::Deactivate => detector.reset_wake(),
                    GateAction::Feed(samples) => {
                        if let Some(detection) =
                            detector.accept_wake(capture.sample_rate(), samples.as_ref())
                        {
                            eprintln!(
                                "wake phrase detected: {} | listening for {} seconds",
                                detection.keyword,
                                LISTEN_WINDOW.as_secs()
                            );

                            listening_until = Some(Instant::now() + LISTEN_WINDOW);
                            notice = Some(notify::ListeningNotice::start(LISTEN_WINDOW.as_secs()));
                            ignore_open_until = Some(Instant::now() + WAKE_ECHO_GUARD);
                            detector.reset_command();
                            gate.reset();

                            // The wake result can arrive after the next command has already
                            // started. Replay the short tail after the wake phrase into the
                            // command stream before switching to live audio. OPEN is skipped:
                            // the wake phrase itself ends in "OpenCode", which is the most
                            // likely source of an immediate false OPEN.
                            let mut handled = false;
                            for chunk in detection.wake_tail.chunks(AUDIO_CHUNK) {
                                if let Some(command) =
                                    detector.accept_command(capture.sample_rate(), chunk)
                                {
                                    if command.command == VoiceCommand::Open {
                                        eprintln!(
                                            "ignored OPEN from the wake phrase; still listening"
                                        );
                                        continue;
                                    }

                                    listening_until = None;
                                    notice = None;
                                    ignore_open_until = None;
                                    eprintln!(
                                        "command detected: {} ({})",
                                        command.keyword,
                                        command_name(command.command)
                                    );
                                    execute_and_cooldown(
                                        command.command,
                                        &config,
                                        &mut detector,
                                        &mut capture,
                                        &mut buffer,
                                    );
                                    handled = true;
                                    break;
                                }
                            }

                            if handled {
                                continue;
                            }
                        }
                    }
                }
            }
        }

        if listening_until.is_some_and(|deadline| Instant::now() >= deadline) {
            listening_until = None;
            notice = None;
            ignore_open_until = None;
            eprintln!(
                "no command within {} seconds; opening OpenCode",
                LISTEN_WINDOW.as_secs()
            );
            execute_and_cooldown(
                VoiceCommand::Open,
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

    detector.reset_all();
    drain(capture, buffer);

    if !cooldown.is_zero() {
        thread::sleep(cooldown);
        drain(capture, buffer);
    }
}

fn execute(command: VoiceCommand, config: &Config) -> Duration {
    match command {
        VoiceCommand::Wake | VoiceCommand::Open => {
            launch(LaunchMode::New, config);
            config.cooldown
        }
        VoiceCommand::Continue => {
            launch(LaunchMode::Continue, config);
            config.cooldown
        }
        VoiceCommand::Help => {
            launch(LaunchMode::Help, config);
            config.cooldown
        }
        VoiceCommand::Stop => {
            stop(config);
            STOP_COOLDOWN
        }
        VoiceCommand::NeverMind => {
            eprintln!("never mind: canceled the listening window");
            Duration::ZERO
        }
    }
}

fn launch(mode: LaunchMode, config: &Config) {
    let action = match mode {
        LaunchMode::New => "open OpenCode",
        LaunchMode::Continue => "open OpenCode with --continue",
        LaunchMode::Help => "open the voice command help",
    };

    if config.dry_run {
        eprintln!("dry run: would {action}");
        return;
    }

    match launcher::launch(mode, config.terminal.as_deref()) {
        Ok(terminal) => eprintln!("launched {action} via {terminal}"),
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

    let mut detector = Detector::new(&config.model_dir, config.threshold, sample_rate as u32)?;
    let mut samples = wave.samples().to_vec();
    let padding = vec![0.0f32; sample_rate as usize * (LISTEN_WINDOW.as_secs() as usize + 1)];
    samples.extend_from_slice(&padding);

    let mut elapsed = Duration::ZERO;
    let mut listening_until: Option<Duration> = None;
    let mut wake_keyword: Option<String> = None;

    for chunk in samples.chunks(AUDIO_CHUNK) {
        if listening_until.is_some() {
            if let Some(detection) = detector.accept_command(sample_rate as u32, chunk) {
                println!(
                    "detected: {} ({})",
                    detection.keyword,
                    command_name(detection.command)
                );
                return Ok(());
            }
        } else if let Some(detection) = detector.accept_wake(sample_rate as u32, chunk) {
            println!("detected: {} (listening)", detection.keyword);
            wake_keyword = Some(detection.keyword);
            listening_until = Some(elapsed + LISTEN_WINDOW);
            detector.reset_command();

            for tail_chunk in detection.wake_tail.chunks(AUDIO_CHUNK) {
                if let Some(command) = detector.accept_command(sample_rate as u32, tail_chunk) {
                    println!(
                        "detected: {} ({})",
                        command.keyword,
                        command_name(command.command)
                    );
                    return Ok(());
                }
            }
        }

        elapsed += Duration::from_secs_f64(chunk.len() as f64 / sample_rate as f64);

        if listening_until.is_some_and(|deadline| elapsed >= deadline) {
            let keyword = wake_keyword.take().unwrap_or_default();
            println!("detected: {keyword} (open after 5s)");
            return Ok(());
        }
    }

    println!("no keyword detected");
    Ok(())
}

fn command_name(command: VoiceCommand) -> &'static str {
    match command {
        VoiceCommand::Wake => "listen",
        VoiceCommand::Open => "open",
        VoiceCommand::Continue => "continue",
        VoiceCommand::Stop => "stop",
        VoiceCommand::Help => "help",
        VoiceCommand::NeverMind => "never mind",
    }
}

fn drain(capture: &mut audio::Capture, buffer: &mut [f32]) {
    while capture.read(buffer) > 0 {}
}
