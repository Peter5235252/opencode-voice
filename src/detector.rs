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
▁CON T IN U E #0.15
▁ST O P #0.15
▁HELP #0.08
";

const ENCODER: &str = "encoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const DECODER: &str = "decoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const JOINER: &str = "joiner-epoch-12-avg-2-chunk-16-left-64.onnx";
const TOKENS: &str = "tokens.txt";

/// Enough history to cover the gap between the end of the wake phrase and the
/// delayed keyword result, without keeping a large always-on buffer.
const HISTORY_SECONDS: usize = 3;
const TAIL_BEFORE_END: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCommand {
    Wake,
    Open,
    Continue,
    Stop,
    Help,
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
        let sample_rate = sample_rate.max(8_000);

        Ok(Self {
            spotter,
            wake_stream,
            command_stream,
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

    pub fn reset_all(&mut self) {
        self.reset_wake();
        self.reset_command();
    }

    fn remember(&mut self, samples: &[f32]) {
        self.wake_samples += samples.len();
        self.history.extend(samples.iter().copied());
        while self.history.len() > self.history_capacity {
            self.history.pop_front();
        }
    }

    fn wake_tail(&self, last_timestamp: Option<f32>) -> Vec<f32> {
        // Timestamps are relative to the last stream reset. Convert the last
        // wake token to a sample index and include a tiny overlap so the start
        // of a command is not clipped.
        let history_start = self.wake_samples.saturating_sub(self.history.len());
        let base_end = last_timestamp
            .filter(|timestamp| timestamp.is_finite() && *timestamp >= 0.0)
            .map(|timestamp| (timestamp * self.sample_rate as f32).round() as usize)
            .unwrap_or_else(|| {
                self.wake_samples
                    .saturating_sub(self.sample_rate as usize / 2)
            })
            .min(self.wake_samples);

        let overlap = (self.sample_rate as f32 * TAIL_BEFORE_END) as usize;
        let start = base_end.saturating_sub(overlap).max(history_start);
        let skip = start.saturating_sub(history_start);

        self.history.iter().skip(skip).copied().collect()
    }
}

fn classify_command(keyword: &str) -> Option<VoiceCommand> {
    let normalized: String = keyword
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .flat_map(char::to_lowercase)
        .collect();

    match normalized.as_str() {
        "open" => Some(VoiceCommand::Open),
        "continue" => Some(VoiceCommand::Continue),
        "stop" => Some(VoiceCommand::Stop),
        "help" => Some(VoiceCommand::Help),
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
        assert!(COMMAND_KEYWORDS.contains("▁CON T IN U E"));
        assert!(COMMAND_KEYWORDS.contains("▁ST O P"));
        assert!(COMMAND_KEYWORDS.contains("▁HELP #0.08"));
        assert!(COMMAND_KEYWORDS.ends_with('\n'));
    }

    #[test]
    fn classifies_all_commands() {
        assert_eq!(classify_command("OPEN"), Some(VoiceCommand::Open));
        assert_eq!(classify_command("CONTINUE"), Some(VoiceCommand::Continue));
        assert_eq!(classify_command("STOP"), Some(VoiceCommand::Stop));
        assert_eq!(classify_command("HELP"), Some(VoiceCommand::Help));
        assert_eq!(classify_command("something else"), None);
    }
}
