use std::path::{Path, PathBuf};

use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};

/// sherpa-onnx keywords are written as sentencepiece/BPE pieces separated by
/// spaces. The first group covers "open code" and "opencode", plus the two most
/// common near-miss pronunciations. The standalone command words are accepted
/// only during the short follow-up window after a wake phrase, so saying
/// "continue" or "stop" in normal conversation does not trigger anything.
///
/// Keep the final newline.
const KEYWORDS: &str = "\
▁HE Y ▁O P EN ▁CO DE
▁HE Y ▁O P EN CO DE
▁HE Y ▁O P EN ▁CO AT
▁HE Y ▁O P EN ▁CO DER
▁CON T IN U E
▁ST O P
";

const ENCODER: &str = "encoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const DECODER: &str = "decoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const JOINER: &str = "joiner-epoch-12-avg-2-chunk-16-left-64.onnx";
const TOKENS: &str = "tokens.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCommand {
    Launch,
    Continue,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub command: VoiceCommand,
    pub keyword: String,
}

pub struct Detector {
    spotter: KeywordSpotter,
    stream: OnlineStream,
}

impl Detector {
    pub fn new(model_dir: &Path, threshold: f32) -> Result<Self, String> {
        let encoder = model_file(model_dir, ENCODER)?;
        let decoder = model_file(model_dir, DECODER)?;
        let joiner = model_file(model_dir, JOINER)?;
        let tokens = model_file(model_dir, TOKENS)?;

        let mut config = KeywordSpotterConfig::default();
        config.model_config.transducer.encoder = Some(encoder);
        config.model_config.transducer.decoder = Some(decoder);
        config.model_config.transducer.joiner = Some(joiner);
        config.model_config.tokens = Some(tokens);
        config.model_config.num_threads = 1;
        config.model_config.provider = Some("cpu".to_string());
        config.keywords_buf = Some(KEYWORDS.to_string());
        config.keywords_score = 1.0;
        config.keywords_threshold = threshold;

        let spotter = KeywordSpotter::create(&config).ok_or_else(|| {
            format!(
                "could not create the keyword spotter from {}",
                model_dir.display()
            )
        })?;
        let stream = spotter.create_stream();

        Ok(Self { spotter, stream })
    }

    /// Feed mono f32 samples. Returns the matched keyword once a phrase is
    /// found. The stream is reset automatically after a detection.
    pub fn accept(&mut self, sample_rate: u32, samples: &[f32]) -> Option<Detection> {
        if samples.is_empty() {
            return None;
        }

        self.stream.accept_waveform(sample_rate as i32, samples);

        while self.spotter.is_ready(&self.stream) {
            self.spotter.decode(&self.stream);

            if let Some(result) = self.spotter.get_result(&self.stream) {
                if !result.keyword.is_empty() {
                    self.spotter.reset(&self.stream);
                    return Some(Detection {
                        command: classify(&result.keyword),
                        keyword: result.keyword,
                    });
                }
            }
        }

        None
    }

    pub fn reset(&mut self) {
        self.spotter.reset(&self.stream);
    }
}

/// Map the decoded keyword back to an action. Any keyword other than the two
/// command words belongs to the "Hey, OpenCode" group, including the near-miss
/// spellings.
fn classify(keyword: &str) -> VoiceCommand {
    let normalized: String = keyword
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .flat_map(char::to_lowercase)
        .collect();

    if normalized.contains("continue") {
        VoiceCommand::Continue
    } else if normalized.contains("stop") {
        VoiceCommand::Stop
    } else {
        VoiceCommand::Launch
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
        assert_eq!(KEYWORDS.lines().count(), 6);
        assert!(KEYWORDS.contains("▁HE Y"));
        assert!(KEYWORDS.contains("▁CON T IN U E"));
        assert!(KEYWORDS.contains("▁ST O P"));
        assert!(KEYWORDS.ends_with('\n'));
    }

    #[test]
    fn classifies_wake_variants() {
        for keyword in [
            "HEY OPEN CODE",
            "HEY OPENCODE",
            "HEY OPEN COAT",
            "HEY OPEN CODER",
        ] {
            assert_eq!(classify(keyword), VoiceCommand::Launch, "{keyword}");
        }
    }

    #[test]
    fn classifies_commands() {
        assert_eq!(classify("CONTINUE"), VoiceCommand::Continue);
        assert_eq!(classify("STOP"), VoiceCommand::Stop);
        assert_eq!(classify("HEY OPEN CODE CONTINUE"), VoiceCommand::Continue);
        assert_eq!(classify("HEY OPEN CODE STOP"), VoiceCommand::Stop);
    }
}
