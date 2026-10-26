use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::thread;

const HELP_SCRIPT: &str = include_str!("../scripts/opencode-voice-help.sh");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    New,
    Continue,
    Help,
}

pub fn launch(mode: LaunchMode) -> Result<(), String> {
    let mut command = Command::new("konsole");

    match mode {
        LaunchMode::New => {
            command.args(["--separate", "--title", "OpenCode", "-e", "opencode"]);
        }
        LaunchMode::Continue => {
            command.args([
                "--separate",
                "--title",
                "OpenCode",
                "-e",
                "opencode",
                "--continue",
            ]);
        }
        LaunchMode::Help => {
            command.args(["--separate", "--title", "OpenCode Voice Help", "-e"]);
            match help_script() {
                Some(script) => {
                    command.arg(script);
                }
                None => {
                    command.args(["bash", "-c", HELP_SCRIPT]);
                }
            }
        }
    }

    if let Some(home) = env::var_os("HOME") {
        command.current_dir(home);
    }

    let mut child = command
        .spawn()
        .map_err(|err| format!("could not start Konsole: {err}"))?;

    // Reap the child eventually without blocking the detector loop.
    thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

fn help_script() -> Option<PathBuf> {
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            let script = directory.join("opencode-voice-help");
            if script.is_file() {
                return Some(script);
            }
        }
    }

    let script = PathBuf::from(env::var_os("HOME")?)
        .join(".local")
        .join("bin")
        .join("opencode-voice-help");

    script.is_file().then_some(script)
}
