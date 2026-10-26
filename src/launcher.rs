use std::env;
use std::process::Command;
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    New,
    Continue,
}

pub fn launch(mode: LaunchMode) -> Result<(), String> {
    let mut command = Command::new("konsole");
    command.args(["--separate", "--title", "OpenCode", "-e", "opencode"]);

    if mode == LaunchMode::Continue {
        command.arg("--continue");
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
