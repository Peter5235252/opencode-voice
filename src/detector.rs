use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};

/// The wake stream only contains the wake phrase. The command stream is fed
/// only during the five-second listening window, so short words such as "open"
/// cannot trigger inside "OpenCode".
const WAKE_KEYWORDS: &str = "\
▁HE Y ▁O P EN ▁CO DE
▁HE Y ▁O P EN CO DE
▁HE Y ▁O P EN ▁CO AT
▁HE Y ▁O P EN ▁CO DER
";

/// Command words use per-keyword thresholds. "help" is a single short token and
/// needs a slightly lower threshold than the longer commands.
const COMMAND_KEYWORDS: &str = "\
▁O P EN #0.15
▁O P EN ▁MIN I #0.05
▁MIN I #0.05
▁O P EN ▁CHA T G P T #0.05
▁O P EN ▁CHA T ▁G P T #0.05
▁O P EN ▁CHA T ▁G P #0.05
▁CHA T G P T #0.08
▁CHA T ▁G P T #0.08
▁CHA T ▁G P #0.08
▁O P EN ▁C LA U DE #0.05
▁C LA #0.05
▁CON T IN U E #0.15
▁RE S U ME #0.15
▁ST O P #0.15
▁HELP #0.08
▁NEVER ▁MIN D #0.10
▁NE VER ▁MIN D #0.10
";

const MINI_KEYWORDS: &str = "▁MIN I #0.05\n";

/// Used only after the OPEN command has been heard. Short prefixes let "open
/// mini", "open ChatGPT", and "open Claude" resolve before the plain OPEN
/// timeout.
const PENDING_KEYWORDS: &str = "\
▁MIN #0.05
▁CHA #0.05
▁CHA T #0.05
▁C LA #0.05
▁C L O #0.05
";

const ENCODER: &str = "encoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const DECODER: &str = "decoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const JOINER: &str = "joiner-epoch-12-avg-2-chunk-16-left-64.onnx";
const TOKENS: &str = "tokens.txt";

/// Enough history to cover the gap between the end of the wake phrase and the
/// delayed keyword result, without keeping a large always-on buffer.
const HISTORY_SECONDS: usize = 3;

/// Start the replay a little before the last wake token so the first phoneme of
/// an immediately spoken command is not clipped. This is safe as long as the
/// replay never starts near the "open" part of the wake phrase; the fallback
/// below skips the replay entirely when no timestamp is available.
const TAIL_BEFORE_END: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCommand {
    Wake,
    Open,
    Mini,
    ChatGpt,
    Claude,
    Continue,
    Stop,
    Help,
    NeverMind,
}

#[derive(Debug, Clone)]
pub struct Detection {
    pub command: VoiceCommand,
    pub keyword: String,
    /// Audio from just after the wake phrase to the detection point. Feeding it
    /// to the command stream catches commands spoken while the wake result was
    /// still being decoded.
    pub wake_tail: Vec<f32>,
}

pub struct Detector {
    spotter: KeywordSpotter,
    wake_stream: OnlineStream,
    command_stream: OnlineStream,
    mini_stream: OnlineStream,
    pending_stream: OnlineStream,
    sample_rate: u32,
    wake_samples: usize,
    history: VecDeque<f32>,
    history_capacity: usize,
}

impl Detector {
    pub fn new(model_dir: &Path, threshold: f32, sample_rate: u32) -> Result<Self, String> {
        let encoder = model_file(model_dir, ENCODER)?;
        let decoder = model_file(model_dir, DECODER)?;
        let joiner = model_file(model_dir, JOINER)?;
        let tokens = model_file(model_dir, TOKENS)?;

        let mut config = KeywordSpotterConfig::default();
        config.model_config.transducer.encoder = Some(encoder);
        config.model_config.transducer.decoder = Some(decoder);
        config.model_config.transducer.joiner = Some(joiner);
        config.model_config.tokens = Some(tokens);
        // This specific model is a streaming Zipformer2 transducer. Telling
        // sherpa-onnx avoids its model-type probing and duplicate load.
        config.model_config.model_type = Some("zipformer2".to_string());
        config.model_config.num_threads = 1;
        config.model_config.provider = Some("cpu".to_string());
        config.keywords_buf = Some(WAKE_KEYWORDS.to_string());
        config.keywords_score = 1.0;
        config.keywords_threshold = threshold;

        let spotter = KeywordSpotter::create(&config).ok_or_else(|| {
            format!(
                "could not create the keyword spotter from {}",
                model_dir.display()
            )
        })?;
        let wake_stream = spotter.create_stream();
        let command_stream = spotter.create_stream_with_keywords(COMMAND_KEYWORDS);
        let mini_stream = spotter.create_stream_with_keywords(MINI_KEYWORDS);
        let pending_stream = spotter.create_stream_with_keywords(PENDING_KEYWORDS);
        let sample_rate = sample_rate.max(8_000);

        Ok(Self {
            spotter,
            wake_stream,
            command_stream,
            mini_stream,
            pending_stream,
            sample_rate,
            wake_samples: 0,
            history: VecDeque::with_capacity(sample_rate as usize * HISTORY_SECONDS),
            history_capacity: sample_rate as usize * HISTORY_SECONDS,
        })
    }

    /// Feed audio to the always-on wake stream. On a wake phrase, returns the
    /// phrase plus the recent audio that follows its end.
    pub fn accept_wake(&mut self, sample_rate: u32, samples: &[f32]) -> Option<Detection> {
        if samples.is_empty() {
            return None;
        }

        self.wake_stream
            .accept_waveform(sample_rate as i32, samples);
        self.remember(samples);

        while self.spotter.is_ready(&self.wake_stream) {
            self.spotter.decode(&self.wake_stream);

            if let Some(result) = self.spotter.get_result(&self.wake_stream) {
                if result.keyword.is_empty() {
                    continue;
                }

                let tail = self.wake_tail(result.timestamps.last().copied());
                if std::env::var_os("OPENCODE_VOICE_DEBUG").is_some() {
                    let last_timestamp = result.timestamps.last().copied();
                    let history_start = self.wake_samples.saturating_sub(self.history.len());
                    let tail_start = wake_tail_start(
                        self.wake_samples,
                        history_start,
                        last_timestamp,
                        self.sample_rate,
                    );
                    eprintln!(
                        "debug wake: keyword={:?} timestamps={:?} tokens={:?} wake_samples={} history_start={} tail_start={} tail_samples={}",
                        result.keyword,
                        result.timestamps,
                        result.tokens,
                        self.wake_samples,
                        history_start,
                        tail_start,
                        tail.len()
                    );
                }
                self.spotter.reset(&self.wake_stream);
                self.wake_samples = 0;
                self.history.clear();

                return Some(Detection {
                    command: VoiceCommand::Wake,
                    keyword: result.keyword,
                    wake_tail: tail,
                });
            }
        }

        None
    }

    /// Feed audio to the command stream. The caller should only call this while
    /// the listening window is active.
    pub fn accept_command(&mut self, sample_rate: u32, samples: &[f32]) -> Option<Detection> {
        if samples.is_empty() {
            return None;
        }

        self.command_stream
            .accept_waveform(sample_rate as i32, samples);

        while self.spotter.is_ready(&self.command_stream) {
            self.spotter.decode(&self.command_stream);

            if let Some(result) = self.spotter.get_result(&self.command_stream) {
                if result.keyword.is_empty() {
                    continue;
                }

                let command = classify_command(&result.keyword);
                if std::env::var_os("OPENCODE_VOICE_DEBUG").is_some() {
                    eprintln!(
                        "debug command: keyword={:?} timestamps={:?} tokens={:?} tokens_arr={:?}",
                        result.keyword, result.timestamps, result.tokens, result.tokens_arr
                    );
                }
                self.spotter.reset(&self.command_stream);

                return command.map(|command| Detection {
                    command,
                    keyword: result.keyword,
                    wake_tail: Vec::new(),
                });
            }
        }

        None
    }

    pub fn reset_wake(&mut self) {
        self.spotter.reset(&self.wake_stream);
        self.wake_samples = 0;
        self.history.clear();
    }

    pub fn reset_command(&mut self) {
        self.spotter.reset(&self.command_stream);
    }

    /// Feed audio to the dedicated mini stream. This runs for the whole
    /// listening window so a standalone "mini" is detected even when its start
    /// falls inside the replayed wake tail.
    pub fn accept_mini(&mut self, sample_rate: u32, samples: &[f32]) -> bool {
        if samples.is_empty() {
            return false;
        }

        self.mini_stream
            .accept_waveform(sample_rate as i32, samples);

        while self.spotter.is_ready(&self.mini_stream) {
            self.spotter.decode(&self.mini_stream);

            if let Some(result) = self.spotter.get_result(&self.mini_stream) {
                if result.keyword.is_empty() {
                    continue;
                }

                // Per-stream keyword lists can still see the configured wake
                // words. Ignore anything that is not the mini command.
                if !result.keyword.eq_ignore_ascii_case("MINI") {
                    if std::env::var_os("OPENCODE_VOICE_DEBUG").is_some() {
                        eprintln!(
                            "debug mini ignored: keyword={:?} timestamps={:?} tokens={:?}",
                            result.keyword, result.timestamps, result.tokens
                        );
                    }
                    self.spotter.reset(&self.mini_stream);
                    return false;
                }

                if std::env::var_os("OPENCODE_VOICE_DEBUG").is_some() {
                    eprintln!(
                        "debug mini: keyword={:?} timestamps={:?} tokens={:?}",
                        result.keyword, result.timestamps, result.tokens
                    );
                }
                self.spotter.reset(&self.mini_stream);
                return true;
            }
        }

        false
    }

    /// Feed the short prefix stream used only while an OPEN command is waiting
    /// to see whether the speaker continues with "mini", "ChatGPT", or
    /// "Claude".
    pub fn accept_pending(&mut self, sample_rate: u32, samples: &[f32]) -> Option<VoiceCommand> {
        if samples.is_empty() {
            return None;
        }

        self.pending_stream
            .accept_waveform(sample_rate as i32, samples);

        while self.spotter.is_ready(&self.pending_stream) {
            self.spotter.decode(&self.pending_stream);

            if let Some(result) = self.spotter.get_result(&self.pending_stream) {
                if result.keyword.is_empty() {
                    continue;
                }

                let command = pending_command_for_keyword(&result.keyword);

                if std::env::var_os("OPENCODE_VOICE_DEBUG").is_some() {
                    if command.is_some() {
                        eprintln!(
                            "debug pending: keyword={:?} timestamps={:?} tokens={:?}",
                            result.keyword, result.timestamps, result.tokens
                        );
                    } else {
                        eprintln!(
                            "debug pending ignored: keyword={:?} timestamps={:?} tokens={:?}",
                            result.keyword, result.timestamps, result.tokens
                        );
                    }
                }

                self.spotter.reset(&self.pending_stream);
                return command;
            }
        }

        None
    }

    pub fn reset_mini(&mut self) {
        self.spotter.reset(&self.mini_stream);
    }

    pub fn reset_pending(&mut self) {
        self.spotter.reset(&self.pending_stream);
    }

    pub fn reset_all(&mut self) {
        self.reset_wake();
        self.reset_command();
        self.reset_mini();
        self.reset_pending();
    }

    fn remember(&mut self, samples: &[f32]) {
        self.wake_samples += samples.len();
        self.history.extend(samples.iter().copied());
        while self.history.len() > self.history_capacity {
            self.history.pop_front();
        }
    }

    fn wake_tail(&self, last_timestamp: Option<f32>) -> Vec<f32> {
        let history_start = self.wake_samples.saturating_sub(self.history.len());
        let start = wake_tail_start(
            self.wake_samples,
            history_start,
            last_timestamp,
            self.sample_rate,
        );
        let skip = start.saturating_sub(history_start);

        self.history.iter().skip(skip).copied().collect()
    }
}

/// The sample index where the post-wake replay starts.
///
/// Timestamps are relative to the last stream reset. We replay a small amount
/// before the last wake token so an immediately spoken command is not clipped.
/// If the model does not provide a timestamp, we skip the replay entirely: the
/// old fixed half-second fallback could contain the "open" in "OpenCode" and
/// fire an OPEN command immediately after a plain wake phrase.
fn wake_tail_start(
    wake_samples: usize,
    history_start: usize,
    last_timestamp: Option<f32>,
    sample_rate: u32,
) -> usize {
    let last_token_end = match last_timestamp {
        Some(timestamp) if timestamp.is_finite() && timestamp >= 0.0 => {
            (timestamp * sample_rate as f32).round() as usize
        }
        _ => return wake_samples,
    };

    let overlap = (sample_rate as f32 * TAIL_BEFORE_END) as usize;
    last_token_end
        .saturating_sub(overlap)
        .max(history_start)
        .min(wake_samples)
}

fn normalize_keyword(keyword: &str) -> String {
    keyword
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .flat_map(char::to_lowercase)
        .collect()
}

fn pending_command_for_keyword(keyword: &str) -> Option<VoiceCommand> {
    match normalize_keyword(keyword).as_str() {
        "min" => Some(VoiceCommand::Mini),
        "cha" | "chat" => Some(VoiceCommand::ChatGpt),
        "cla" | "clo" => Some(VoiceCommand::Claude),
        _ => None,
    }
}

fn classify_command(keyword: &str) -> Option<VoiceCommand> {
    match normalize_keyword(keyword).as_str() {
        "open" => Some(VoiceCommand::Open),
        "openmini" | "mini" => Some(VoiceCommand::Mini),
        "openchatgpt" | "chatgp" | "chatgpt" => Some(VoiceCommand::ChatGpt),
        "openclaude" | "claude" => Some(VoiceCommand::Claude),
        "continue" | "resume" => Some(VoiceCommand::Continue),
        "stop" => Some(VoiceCommand::Stop),
        "help" => Some(VoiceCommand::Help),
        "nevermind" => Some(VoiceCommand::NeverMind),
        _ => None,
    }
}

fn model_file(model_dir: &Path, name: &str) -> Result<String, String> {
    let path: PathBuf = model_dir.join(name);
    if !path.is_file() {
        return Err(format!(
            "missing model file: {}\nRun install.sh to download the wake-word model.",
            path.display()
        ));
    }

    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("model path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_bpe_tokenized() {
        assert_eq!(WAKE_KEYWORDS.lines().count(), 4);
        assert!(WAKE_KEYWORDS.contains("▁HE Y"));
        assert!(WAKE_KEYWORDS.ends_with('\n'));

        assert!(COMMAND_KEYWORDS.contains("▁O P EN"));
        assert!(COMMAND_KEYWORDS.contains("▁O P EN ▁MIN I #0.05"));
        assert!(COMMAND_KEYWORDS.contains("▁MIN I #0.05"));
        assert!(COMMAND_KEYWORDS.contains("▁O P EN ▁CHA T G P T #0.05"));
        assert!(COMMAND_KEYWORDS.contains("▁O P EN ▁CHA T ▁G P T #0.05"));
        assert!(COMMAND_KEYWORDS.contains("▁O P EN ▁CHA T ▁G P #0.05"));
        assert!(COMMAND_KEYWORDS.contains("▁CHA T G P T #0.08"));
        assert!(COMMAND_KEYWORDS.contains("▁CHA T ▁G P #0.08"));
        assert!(COMMAND_KEYWORDS.contains("▁O P EN ▁C LA U DE #0.05"));
        assert!(COMMAND_KEYWORDS.contains("▁C LA #0.05"));
        assert!(COMMAND_KEYWORDS.contains("▁CON T IN U E"));
        assert!(COMMAND_KEYWORDS.contains("▁RE S U ME #0.15"));
        assert!(COMMAND_KEYWORDS.contains("▁ST O P"));
        assert!(COMMAND_KEYWORDS.contains("▁HELP #0.08"));
        assert!(COMMAND_KEYWORDS.contains("▁NEVER ▁MIN D"));
        assert!(COMMAND_KEYWORDS.ends_with('\n'));

        assert_eq!(MINI_KEYWORDS, "▁MIN I #0.05\n");
        assert!(PENDING_KEYWORDS.contains("▁MIN #0.05"));
        assert!(PENDING_KEYWORDS.contains("▁CHA #0.05"));
        assert!(PENDING_KEYWORDS.contains("▁CHA T #0.05"));
        assert!(PENDING_KEYWORDS.contains("▁C LA #0.05"));
        assert!(PENDING_KEYWORDS.contains("▁C L O #0.05"));

        assert_eq!(pending_command_for_keyword("MIN"), Some(VoiceCommand::Mini));
        assert_eq!(
            pending_command_for_keyword("CHA"),
            Some(VoiceCommand::ChatGpt)
        );
        assert_eq!(
            pending_command_for_keyword("CHAT"),
            Some(VoiceCommand::ChatGpt)
        );
        assert_eq!(
            pending_command_for_keyword("C LA"),
            Some(VoiceCommand::Claude)
        );
        assert_eq!(
            pending_command_for_keyword("C LO"),
            Some(VoiceCommand::Claude)
        );
        assert_eq!(pending_command_for_keyword("HEY OPENCODE"), None);
    }

    #[test]
    fn classifies_all_commands() {
        assert_eq!(classify_command("OPEN"), Some(VoiceCommand::Open));
        assert_eq!(classify_command("OPEN MINI"), Some(VoiceCommand::Mini));
        assert_eq!(classify_command("MINI"), Some(VoiceCommand::Mini));
        assert_eq!(
            classify_command("OPEN CHATGPT"),
            Some(VoiceCommand::ChatGpt)
        );
        assert_eq!(classify_command("CHATGPT"), Some(VoiceCommand::ChatGpt));
        assert_eq!(classify_command("CHAT GP"), Some(VoiceCommand::ChatGpt));
        assert_eq!(classify_command("OPEN CLAUDE"), Some(VoiceCommand::Claude));
        assert_eq!(classify_command("CLAUDE"), Some(VoiceCommand::Claude));
        assert_eq!(classify_command("CONTINUE"), Some(VoiceCommand::Continue));
        assert_eq!(classify_command("RESUME"), Some(VoiceCommand::Continue));
        assert_eq!(classify_command("resume"), Some(VoiceCommand::Continue));
        assert_eq!(classify_command("STOP"), Some(VoiceCommand::Stop));
        assert_eq!(classify_command("HELP"), Some(VoiceCommand::Help));
        assert_eq!(
            classify_command("NEVER MIND"),
            Some(VoiceCommand::NeverMind)
        );
        assert_eq!(classify_command("nevermind"), Some(VoiceCommand::NeverMind));
        assert_eq!(classify_command("something else"), None);
    }

    #[test]
    fn wake_tail_starts_slightly_before_the_last_wake_token() {
        // 1.0 s timestamp at 16 kHz, minus the 50 ms anti-clipping overlap.
        assert_eq!(wake_tail_start(32_000, 0, Some(1.0), 16_000), 16_000 - 800);

        // No usable timestamp: do not replay any audio from the wake phrase.
        assert_eq!(wake_tail_start(32_000, 0, None, 16_000), 32_000);
        assert_eq!(wake_tail_start(32_000, 0, Some(f32::NAN), 16_000), 32_000);
        assert_eq!(wake_tail_start(32_000, 0, Some(-1.0), 16_000), 32_000);

        // Never start before the retained history.
        assert_eq!(wake_tail_start(32_000, 10_000, Some(0.5), 16_000), 10_000);
    }
}
