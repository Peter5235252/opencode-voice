use std::env;
use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "\
opencode-voice - launch or control OpenCode with your voice

VOICE COMMANDS:
    \"Hey, OpenCode\"             Listen for 5 seconds, then open OpenCode
    \"Hey, OpenCode, open\"       Open OpenCode immediately
    \"Hey, OpenCode, continue\"   Resume the last session (opencode --continue)
    \"Hey, OpenCode, stop\"       Interrupt the active OpenCode response
    \"Hey, OpenCode, help\"       Show the voice command help in a new terminal

USAGE:
    opencode-voice [OPTIONS]

OPTIONS:
    --model-dir <DIR>   Directory containing the sherpa-onnx KWS model
    --device <NAME>     Input device name (case-insensitive substring)
    --terminal <NAME>   Terminal emulator to use (default: auto-detect)
    --threshold <0-1>   Keyword confidence threshold (default: 0.15)
    --energy-gate <RMS> Skip wake decoding below this RMS (0 disables, default: 0.003)
    --cooldown <SECS>   Ignore repeated launches for this long (default: 4)
    --wav <FILE>        Test a WAV file instead of listening to the microphone
    --dry-run           Detect and log, but do not launch a terminal
    --list-devices      List available audio input devices and exit
    -h, --help          Print this help and exit
    -V, --version       Print the version and exit

ENVIRONMENT:
    XDG_DATA_HOME       Used to find the default model directory
    OPENCODE_VOICE_TERMINAL
                        Preferred terminal emulator; same names as --terminal

The microphone audio never leaves this machine. The wake-word model runs
entirely offline through sherpa-onnx.
";

#[derive(Debug, Clone)]
pub struct Config {
    pub model_dir: PathBuf,
    pub device: Option<String>,
    pub terminal: Option<String>,
    pub threshold: f32,
    pub energy_gate: f32,
    pub cooldown: Duration,
    pub wav: Option<PathBuf>,
    pub dry_run: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model_dir: default_model_dir(),
            device: None,
            terminal: None,
            threshold: 0.15,
            energy_gate: 0.003,
            cooldown: Duration::from_secs(4),
            wav: None,
            dry_run: false,
        }
    }
}

#[derive(Debug)]
pub enum Command {
    Run(Config),
    ListDevices,
    Help,
    Version,
}

impl Config {
    pub fn parse<I>(args: I) -> Result<Command, String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut config = Self::default();
        let mut args = args.into_iter();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => return Ok(Command::Help),
                "-V" | "--version" => return Ok(Command::Version),
                "--list-devices" => return Ok(Command::ListDevices),
                "--dry-run" => config.dry_run = true,
                "--model-dir" => {
                    config.model_dir = PathBuf::from(next_value(&mut args, "--model-dir")?);
                }
                "--device" => {
                    config.device = Some(next_value(&mut args, "--device")?);
                }
                "--terminal" => {
                    let value = next_value(&mut args, "--terminal")?;
                    config.terminal = if value.eq_ignore_ascii_case("auto") {
                        None
                    } else {
                        Some(value)
                    };
                }
                "--threshold" => {
                    let value = next_value(&mut args, "--threshold")?;
                    config.threshold = parse_threshold(&value)?;
                }
                "--energy-gate" => {
                    let value = next_value(&mut args, "--energy-gate")?;
                    config.energy_gate = parse_energy_gate(&value)?;
                }
                "--cooldown" => {
                    let value = next_value(&mut args, "--cooldown")?;
                    config.cooldown = parse_cooldown(&value)?;
                }
                "--wav" => {
                    config.wav = Some(PathBuf::from(next_value(&mut args, "--wav")?));
                }
                other => {
                    return Err(format!("unknown argument: {other}"));
                }
            }
        }

        Ok(Command::Run(config))
    }
}

fn next_value<I>(args: &mut I, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_threshold(value: &str) -> Result<f32, String> {
    let threshold = value
        .parse::<f32>()
        .map_err(|_| format!("invalid threshold: {value}"))?;
    if !(0.0..=1.0).contains(&threshold) {
        return Err(format!("threshold must be between 0 and 1: {value}"));
    }
    Ok(threshold)
}

fn parse_energy_gate(value: &str) -> Result<f32, String> {
    let threshold = value
        .parse::<f32>()
        .map_err(|_| format!("invalid energy gate: {value}"))?;
    if !(0.0..=1.0).contains(&threshold) {
        return Err(format!("energy gate must be between 0 and 1: {value}"));
    }
    Ok(threshold)
}

fn parse_cooldown(value: &str) -> Result<Duration, String> {
    let seconds = value
        .parse::<f32>()
        .map_err(|_| format!("invalid cooldown: {value}"))?;
    if !(0.0..=60.0).contains(&seconds) {
        return Err(format!(
            "cooldown must be between 0 and 60 seconds: {value}"
        ));
    }
    Ok(Duration::from_secs_f32(seconds))
}

pub fn default_model_dir() -> PathBuf {
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));

    data_home
        .join("opencode-voice")
        .join("models")
        .join("sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_defaults() {
        let command = Config::parse(args(&[])).expect("default config should parse");
        let Command::Run(config) = command else {
            panic!("expected run command");
        };

        assert_eq!(config.threshold, 0.15);
        assert_eq!(config.energy_gate, 0.003);
        assert_eq!(config.cooldown, Duration::from_secs(4));
        assert!(config.device.is_none());
        assert!(config.terminal.is_none());
        assert!(config.wav.is_none());
        assert!(!config.dry_run);
    }

    #[test]
    fn parses_overrides() {
        let command = Config::parse(args(&[
            "--device",
            "USB Mic",
            "--terminal",
            "alacritty",
            "--threshold",
            "0.25",
            "--energy-gate",
            "0.01",
            "--cooldown",
            "2.5",
            "--dry-run",
        ]))
        .expect("config should parse");

        let Command::Run(config) = command else {
            panic!("expected run command");
        };

        assert_eq!(config.device.as_deref(), Some("USB Mic"));
        assert_eq!(config.terminal.as_deref(), Some("alacritty"));
        assert_eq!(config.threshold, 0.25);
        assert_eq!(config.energy_gate, 0.01);
        assert_eq!(config.cooldown, Duration::from_secs_f32(2.5));
        assert!(config.dry_run);
    }

    #[test]
    fn terminal_auto_clears_override() {
        let command = Config::parse(args(&["--terminal", "auto"])).expect("config should parse");
        let Command::Run(config) = command else {
            panic!("expected run command");
        };

        assert!(config.terminal.is_none());
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(Config::parse(args(&["--threshold", "2"])).is_err());
        assert!(Config::parse(args(&["--energy-gate", "-1"])).is_err());
        assert!(Config::parse(args(&["--cooldown", "-1"])).is_err());
        assert!(Config::parse(args(&["--device"])).is_err());
        assert!(Config::parse(args(&["--terminal"])).is_err());
        assert!(Config::parse(args(&["--nope"])).is_err());
    }
}
