use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// A cancellable desktop-notification countdown. The notification is updated
/// once per second in a background thread so the audio loop never blocks.
pub struct ListeningNotice {
    cancel: Arc<AtomicBool>,
}

impl ListeningNotice {
    pub fn start(seconds: u64) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);

        thread::spawn(move || {
            show_countdown(seconds, flag);
        });

        Self { cancel }
    }
}

impl Drop for ListeningNotice {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn show_countdown(seconds: u64, cancel: Arc<AtomicBool>) {
    let mut notification_id: Option<String> = None;

    for remaining in (1..=seconds).rev() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }

        let body = format!("I'm listening... {remaining}\nSay open, continue, stop, or help.");

        let mut command = Command::new("notify-send");
        command.args([
            "--app-name=opencode-voice",
            "--urgency=normal",
            "--expire-time=1500",
            "--print-id",
        ]);
        if let Some(id) = &notification_id {
            command.args(["--replace-id", id.as_str()]);
        }
        command.arg("OpenCode Voice").arg(body);
        command.stdout(Stdio::piped()).stderr(Stdio::null());

        match command.output() {
            Ok(output) if output.status.success() => {
                let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !printed.is_empty() {
                    notification_id = Some(printed);
                }
            }
            Ok(_) => return,
            Err(error) => {
                eprintln!("could not show listening notification: {error}");
                return;
            }
        }

        // Sleep in small steps so a detected command can cancel the countdown
        // without waiting for the full second.
        for _ in 0..10 {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}
