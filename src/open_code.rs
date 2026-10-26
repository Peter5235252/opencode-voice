use std::process::Command;

use serde_json::Value;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StopReport {
    pub active: usize,
    pub interrupted: usize,
    pub errors: Vec<String>,
}

/// Ask the OpenCode background service to interrupt every session that is
/// currently running. This is what the TUI's stop key does, but it works even
/// when the Konsole window is not focused.
pub fn stop_active_sessions() -> Result<StopReport, String> {
    let output = opencode_api(&["api", "GET", "/api/session/active"])?;
    let session_ids = parse_active_sessions(&output)?;

    let mut report = StopReport {
        active: session_ids.len(),
        ..StopReport::default()
    };

    for session_id in session_ids {
        let path = format!("/api/session/{session_id}/interrupt");
        match opencode_api(&["api", "POST", path.as_str()]) {
            Ok(interrupted) => {
                if parse_interrupted(&interrupted) {
                    report.interrupted += 1;
                }
            }
            Err(error) => report.errors.push(error),
        }
    }

    Ok(report)
}

fn opencode_api(args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("opencode")
        .args(args)
        .output()
        .map_err(|error| format!("could not run opencode: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(format!(
            "opencode {} failed{}{}",
            args.join(" "),
            if stderr.is_empty() { "" } else { ": " },
            stderr
        ));
    }

    Ok(output.stdout)
}

fn parse_active_sessions(output: &[u8]) -> Result<Vec<String>, String> {
    let response: Value = serde_json::from_slice(output)
        .map_err(|error| format!("invalid response from opencode: {error}"))?;
    let data = response
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| "opencode did not return an active-session map".to_string())?;

    Ok(data.keys().cloned().collect())
}

fn parse_interrupted(output: &[u8]) -> bool {
    serde_json::from_slice::<Value>(output)
        .ok()
        .and_then(|response| response.get("interrupted").and_then(Value::as_bool))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_active_sessions() {
        let response = br#"{"data":{"ses_a":{"type":"running"},"ses_b":{"type":"running"}}}"#;
        let mut sessions = parse_active_sessions(response).expect("valid response");
        sessions.sort();
        assert_eq!(sessions, vec!["ses_a".to_string(), "ses_b".to_string()]);
    }

    #[test]
    fn parses_empty_active_sessions() {
        assert!(parse_active_sessions(br#"{"data":{}}"#)
            .expect("valid response")
            .is_empty());
    }

    #[test]
    fn rejects_invalid_active_session_response() {
        assert!(parse_active_sessions(br#"{"oops":true}"#).is_err());
        assert!(parse_active_sessions(b"not json").is_err());
    }

    #[test]
    fn parses_interrupt_results() {
        assert!(parse_interrupted(br#"{"interrupted":true}"#));
        assert!(!parse_interrupted(br#"{"interrupted":false}"#));
        assert!(!parse_interrupted(b"not json"));
    }
}
