use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

const HELP_SCRIPT: &str = include_str!("../scripts/opencode-voice-help.sh");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    New,
    Continue,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalKind {
    Kitty,
    Ghostty,
    Alacritty,
    Konsole,
    Gnome,
    WezTerm,
    Ptyxis,
    Foot,
    Xfce,
    Tilix,
    Terminator,
    Xterm,
    GenericDashE,
    XdgExec,
}

struct TerminalDefinition {
    command: &'static str,
    aliases: &'static [&'static str],
    kind: TerminalKind,
}

/// Known terminals in auto-detection order. The first installed one wins after
/// the user's environment and desktop preferences have been considered.
const TERMINALS: &[TerminalDefinition] = &[
    TerminalDefinition {
        command: "kitty",
        aliases: &["kitty", "xterm-kitty"],
        kind: TerminalKind::Kitty,
    },
    TerminalDefinition {
        command: "ghostty",
        aliases: &["ghostty", "xterm-ghostty", "com.mitchellh.ghostty"],
        kind: TerminalKind::Ghostty,
    },
    TerminalDefinition {
        command: "alacritty",
        aliases: &["alacritty"],
        kind: TerminalKind::Alacritty,
    },
    TerminalDefinition {
        command: "konsole",
        aliases: &["konsole"],
        kind: TerminalKind::Konsole,
    },
    TerminalDefinition {
        command: "gnome-terminal",
        aliases: &[
            "gnome-terminal",
            "gnome-terminal-server",
            "org.gnome.terminal",
        ],
        kind: TerminalKind::Gnome,
    },
    TerminalDefinition {
        command: "wezterm",
        aliases: &["wezterm", "wezterm-gui"],
        kind: TerminalKind::WezTerm,
    },
    TerminalDefinition {
        command: "ptyxis",
        aliases: &["ptyxis"],
        kind: TerminalKind::Ptyxis,
    },
    TerminalDefinition {
        command: "foot",
        aliases: &["foot", "footclient"],
        kind: TerminalKind::Foot,
    },
    TerminalDefinition {
        command: "xfce4-terminal",
        aliases: &["xfce4-terminal"],
        kind: TerminalKind::Xfce,
    },
    TerminalDefinition {
        command: "tilix",
        aliases: &["tilix"],
        kind: TerminalKind::Tilix,
    },
    TerminalDefinition {
        command: "terminator",
        aliases: &["terminator"],
        kind: TerminalKind::Terminator,
    },
    TerminalDefinition {
        command: "xterm",
        aliases: &["xterm"],
        kind: TerminalKind::Xterm,
    },
    TerminalDefinition {
        command: "x-terminal-emulator",
        aliases: &["x-terminal-emulator"],
        kind: TerminalKind::GenericDashE,
    },
    TerminalDefinition {
        command: "xdg-terminal-exec",
        aliases: &["xdg-terminal-exec"],
        kind: TerminalKind::XdgExec,
    },
];

const FALLBACK_ORDER: &[&str] = &[
    "kitty",
    "ghostty",
    "alacritty",
    "konsole",
    "gnome-terminal",
    "wezterm",
    "ptyxis",
    "foot",
    "xfce4-terminal",
    "tilix",
    "terminator",
    "xterm",
    "x-terminal-emulator",
];

struct Terminal {
    program: PathBuf,
    definition: &'static TerminalDefinition,
}

impl Terminal {
    fn name(&self) -> &'static str {
        self.definition.command
    }
}

struct RunCommand {
    program: OsString,
    args: Vec<OsString>,
}

impl RunCommand {
    fn new(program: impl Into<OsString>, args: Vec<OsString>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    fn add_to(&self, command: &mut Command) {
        command.arg(&self.program);
        command.args(&self.args);
    }

    fn shell_line(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(shell_quote(self.program.as_os_str()));
        for arg in &self.args {
            parts.push(shell_quote(arg.as_os_str()));
        }
        parts.join(" ")
    }
}

pub fn launch(mode: LaunchMode, terminal_override: Option<&str>) -> Result<&'static str, String> {
    let terminal = detect_terminal(terminal_override)?;
    let run = run_command(mode);
    let mut command = Command::new(&terminal.program);

    configure_command(
        &mut command,
        terminal.definition.kind,
        title(mode),
        &home_dir(),
        &run,
    );

    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", terminal.name()))?;

    // Reap the child eventually without blocking the detector loop.
    thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(terminal.name())
}

fn detect_terminal(terminal_override: Option<&str>) -> Result<Terminal, String> {
    if let Some(value) = terminal_override {
        return resolve_explicit(value);
    }

    if let Some(value) = env::var_os("OPENCODE_VOICE_TERMINAL") {
        let value = value.to_string_lossy();
        let value = value.trim();
        if !value.is_empty() && !value.eq_ignore_ascii_case("auto") {
            return resolve_explicit(value);
        }
    }

    if let Some(terminal) = terminal_from_env("TERMINAL") {
        return Ok(terminal);
    }
    if let Some(terminal) = terminal_from_hints() {
        return Ok(terminal);
    }
    if let Some(terminal) = terminal_from_environment_program() {
        return Ok(terminal);
    }
    if let Some(terminal) = terminal_from_term() {
        return Ok(terminal);
    }
    if let Some(terminal) = xdg_terminal_exec() {
        return Ok(terminal);
    }

    for name in desktop_preferences() {
        if let Some(terminal) = find_terminal(name) {
            return Ok(terminal);
        }
    }
    for &name in FALLBACK_ORDER {
        if let Some(terminal) = find_terminal(name) {
            return Ok(terminal);
        }
    }

    Err(format!(
        "no supported terminal emulator found. Install one of: {}. Or set --terminal <NAME> / OPENCODE_VOICE_TERMINAL.",
        supported_names().join(", ")
    ))
}

fn resolve_explicit(value: &str) -> Result<Terminal, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("terminal name is empty".to_string());
    }

    let definition = definition_for_alias(value).ok_or_else(|| {
        format!(
            "unsupported terminal emulator '{value}'. Supported: {}",
            supported_names().join(", ")
        )
    })?;
    let program = resolve_program(value, definition).ok_or_else(|| {
        format!("terminal emulator '{value}' was not found in PATH or as an executable path")
    })?;

    Ok(Terminal {
        program,
        definition,
    })
}

fn resolve_program(value: &str, definition: &'static TerminalDefinition) -> Option<PathBuf> {
    if value.contains('/') {
        let path = PathBuf::from(value);
        is_executable(&path).then_some(path)
    } else {
        find_in_path(definition.command)
    }
}

fn terminal_from_env(variable: &str) -> Option<Terminal> {
    let value = env::var_os(variable)?;
    let value = value.to_string_lossy();
    let first = value.split_whitespace().next()?;
    let definition = definition_for_alias(first)?;
    let program = resolve_program(first, definition)?;

    Some(Terminal {
        program,
        definition,
    })
}

fn terminal_from_hints() -> Option<Terminal> {
    if env_any(&["KITTY_PID", "KITTY_WINDOW_ID"]) {
        return find_terminal("kitty");
    }
    if env_any(&["GHOSTTY_RESOURCES_DIR"]) {
        return find_terminal("ghostty");
    }
    if env_any(&["ALACRITTY_SOCKET", "ALACRITTY_WINDOW_ID"]) {
        return find_terminal("alacritty");
    }
    if env_any(&["WEZTERM_PANE"]) {
        return find_terminal("wezterm");
    }
    if env_any(&["KONSOLE_VERSION"]) {
        return find_terminal("konsole");
    }

    None
}

fn env_any(variables: &[&str]) -> bool {
    variables
        .iter()
        .any(|variable| env::var_os(*variable).is_some())
}

fn terminal_from_environment_program() -> Option<Terminal> {
    let value = env::var_os("TERM_PROGRAM")?;
    let value = value.to_string_lossy();
    let definition = definition_for_alias(&value)?;
    let program = resolve_program(&value, definition)?;

    Some(Terminal {
        program,
        definition,
    })
}

fn terminal_from_term() -> Option<Terminal> {
    let term = env::var_os("TERM")?.to_string_lossy().to_ascii_lowercase();
    let alias = if term.contains("kitty") {
        "kitty"
    } else if term.contains("ghostty") {
        "ghostty"
    } else if term.starts_with("foot") {
        "foot"
    } else if term.contains("alacritty") {
        "alacritty"
    } else if term.contains("wezterm") {
        "wezterm"
    } else if term.contains("konsole") {
        "konsole"
    } else if term.contains("xfce") {
        "xfce4-terminal"
    } else if term.contains("tilix") {
        "tilix"
    } else if term.contains("terminator") {
        "terminator"
    } else if term.contains("ptyxis") {
        "ptyxis"
    } else {
        return None;
    };

    find_terminal(alias)
}

fn xdg_terminal_exec() -> Option<Terminal> {
    let definition = terminal_definition("xdg-terminal-exec")?;
    let program = find_in_path("xdg-terminal-exec")?;

    let output = Command::new(&program)
        .arg("--print-cmd")
        .arg("--")
        .arg("true")
        .output()
        .ok()?;

    if output.status.success() && !output.stdout.is_empty() {
        Some(Terminal {
            program,
            definition,
        })
    } else {
        None
    }
}

fn desktop_preferences() -> Vec<&'static str> {
    let desktop = env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();

    if desktop.contains("kde") {
        vec!["konsole"]
    } else if desktop.contains("gnome")
        || desktop.contains("unity")
        || desktop.contains("budgie")
        || desktop.contains("cinnamon")
    {
        vec!["gnome-terminal", "ptyxis"]
    } else if desktop.contains("xfce") {
        vec!["xfce4-terminal"]
    } else {
        Vec::new()
    }
}

fn find_terminal(name: &'static str) -> Option<Terminal> {
    let definition = terminal_definition(name)?;
    let program = find_in_path(name)?;
    Some(Terminal {
        program,
        definition,
    })
}

fn terminal_definition(name: &str) -> Option<&'static TerminalDefinition> {
    TERMINALS
        .iter()
        .find(|definition| definition.command == name)
}

fn definition_for_alias(value: &str) -> Option<&'static TerminalDefinition> {
    let name = value.rsplit('/').next().unwrap_or(value).trim();
    TERMINALS.iter().find(|definition| {
        definition
            .aliases
            .iter()
            .any(|alias| (*alias).eq_ignore_ascii_case(name))
    })
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn supported_names() -> Vec<&'static str> {
    TERMINALS
        .iter()
        .map(|definition| definition.command)
        .collect()
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn run_command(mode: LaunchMode) -> RunCommand {
    match mode {
        LaunchMode::New => RunCommand::new("opencode", Vec::new()),
        LaunchMode::Continue => RunCommand::new("opencode", vec![OsString::from("--continue")]),
        LaunchMode::Help => match help_script() {
            Some(script) => RunCommand::new(script.into_os_string(), Vec::new()),
            None => RunCommand::new(
                "bash",
                vec![OsString::from("-c"), OsString::from(HELP_SCRIPT)],
            ),
        },
    }
}

fn title(mode: LaunchMode) -> &'static str {
    match mode {
        LaunchMode::Help => "OpenCode Voice Help",
        LaunchMode::New | LaunchMode::Continue => "OpenCode",
    }
}

fn configure_command(
    command: &mut Command,
    kind: TerminalKind,
    title: &str,
    workdir: &Path,
    run: &RunCommand,
) {
    let workdir_text = workdir.to_string_lossy();

    command.current_dir(workdir);

    match kind {
        TerminalKind::Kitty => {
            command.args(["--detach", "--title", title, "--directory"]);
            command.arg(workdir.as_os_str());
            run.add_to(command);
        }
        TerminalKind::Ghostty => {
            command.arg(format!("--title={title}"));
            command.arg(format!("--working-directory={workdir_text}"));
            command.arg("-e");
            run.add_to(command);
        }
        TerminalKind::Alacritty => {
            command.args(["--working-directory"]);
            command.arg(workdir.as_os_str());
            command.args(["--title", title, "-e"]);
            run.add_to(command);
        }
        TerminalKind::Konsole => {
            command.args(["--separate", "--workdir"]);
            command.arg(workdir.as_os_str());
            command.args(["--title", title, "-e"]);
            run.add_to(command);
        }
        TerminalKind::Gnome => {
            command.arg("--window");
            command.arg(format!("--title={title}"));
            command.arg(format!("--working-directory={workdir_text}"));
            command.arg("--");
            run.add_to(command);
        }
        TerminalKind::WezTerm => {
            command.args(["start", "--cwd"]);
            command.arg(workdir.as_os_str());
            command.arg("--");
            run.add_to(command);
        }
        TerminalKind::Ptyxis => {
            command.arg("--new-window");
            command.arg(format!("--title={title}"));
            command.arg("--working-directory");
            command.arg(workdir.as_os_str());
            command.arg("--");
            run.add_to(command);
        }
        TerminalKind::Foot => {
            command.arg(format!("--title={title}"));
            command.arg(format!("--working-directory={workdir_text}"));
            run.add_to(command);
        }
        TerminalKind::Xfce => {
            command.arg(format!("--title={title}"));
            command.arg(format!("--working-directory={workdir_text}"));
            command.arg("-x");
            run.add_to(command);
        }
        TerminalKind::Tilix => {
            command.arg(format!("--title={title}"));
            command.arg(format!("--working-directory={workdir_text}"));
            command.arg("-e");
            command.arg(run.shell_line());
        }
        TerminalKind::Terminator => {
            command.arg(format!("--title={title}"));
            command.arg(format!("--working-directory={workdir_text}"));
            command.arg("-x");
            run.add_to(command);
        }
        TerminalKind::Xterm => {
            command.args(["-T", title, "-e"]);
            run.add_to(command);
        }
        TerminalKind::GenericDashE => {
            command.arg("-e");
            run.add_to(command);
        }
        TerminalKind::XdgExec => {
            command.arg(format!("--title={title}"));
            command.arg(format!("--dir={workdir_text}"));
            command.arg("--");
            run.add_to(command);
        }
    }
}

fn shell_quote(value: &OsStr) -> String {
    let text = value.to_string_lossy();
    let safe = !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_./:=@%+,".contains(character));

    if safe {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args_for(kind: TerminalKind, run: &RunCommand) -> Vec<String> {
        let mut command = Command::new("terminal");
        configure_command(
            &mut command,
            kind,
            "OpenCode",
            Path::new("/home/tester"),
            run,
        );

        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    fn continue_run() -> RunCommand {
        RunCommand::new("opencode", vec![OsString::from("--continue")])
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn maps_environment_aliases() {
        assert_eq!(
            definition_for_alias("xterm-kitty").map(|definition| definition.command),
            Some("kitty")
        );
        assert_eq!(
            definition_for_alias("WezTerm").map(|definition| definition.command),
            Some("wezterm")
        );
        assert_eq!(
            definition_for_alias("org.gnome.Terminal").map(|definition| definition.command),
            Some("gnome-terminal")
        );
        assert!(definition_for_alias("vscode").is_none());
    }

    #[test]
    fn builds_kitty_arguments() {
        assert_eq!(
            args_for(TerminalKind::Kitty, &continue_run()),
            strings(&[
                "--detach",
                "--title",
                "OpenCode",
                "--directory",
                "/home/tester",
                "opencode",
                "--continue",
            ])
        );
    }

    #[test]
    fn builds_ghostty_arguments() {
        assert_eq!(
            args_for(TerminalKind::Ghostty, &continue_run()),
            strings(&[
                "--title=OpenCode",
                "--working-directory=/home/tester",
                "-e",
                "opencode",
                "--continue",
            ])
        );
    }

    #[test]
    fn builds_alacritty_arguments() {
        assert_eq!(
            args_for(TerminalKind::Alacritty, &continue_run()),
            strings(&[
                "--working-directory",
                "/home/tester",
                "--title",
                "OpenCode",
                "-e",
                "opencode",
                "--continue",
            ])
        );
    }

    #[test]
    fn builds_konsole_arguments() {
        assert_eq!(
            args_for(TerminalKind::Konsole, &continue_run()),
            strings(&[
                "--separate",
                "--workdir",
                "/home/tester",
                "--title",
                "OpenCode",
                "-e",
                "opencode",
                "--continue",
            ])
        );
    }

    #[test]
    fn builds_gnome_terminal_arguments() {
        assert_eq!(
            args_for(TerminalKind::Gnome, &continue_run()),
            strings(&[
                "--window",
                "--title=OpenCode",
                "--working-directory=/home/tester",
                "--",
                "opencode",
                "--continue",
            ])
        );
    }

    #[test]
    fn builds_wezterm_arguments() {
        assert_eq!(
            args_for(TerminalKind::WezTerm, &continue_run()),
            strings(&[
                "start",
                "--cwd",
                "/home/tester",
                "--",
                "opencode",
                "--continue",
            ])
        );
    }

    #[test]
    fn quotes_commands_for_tilix() {
        let run = RunCommand::new(
            "opencode",
            vec![OsString::from("--title"), OsString::from("two words")],
        );
        assert_eq!(run.shell_line(), "opencode --title 'two words'");
        assert_eq!(shell_quote(OsStr::new("it's")), "'it'\\''s'");
    }
}
