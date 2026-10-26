use std::path::{Path, PathBuf};

use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream};

/// sherpa-onnx keywords are written as sentencepiece/BPE pieces separated by
/// spaces. These four spellings cover "open code" and "opencode", plus the two
/// most common near-miss pronunciations. Keep the final newline.
const KEYWORDS: &str = "\
▁HE Y ▁O P EN ▁CO DE
▁HE Y ▁O P EN CO DE
▁HE Y ▁O P EN ▁CO AT
▁HE Y ▁O P EN ▁CO DER
";

const ENCODER: &str = "encoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const DECODER: &str = "decoder-epoch-12-avg-2-chunk-16-left-64.onnx";
const JOINER: &str = "joiner-epoch-12-avg-2-chunk-16-left-64.onnx";
const TOKENS: &str = "tokens.txt";

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
        config.model_config.model_type = Some("zipformer".to_string());
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

    /// Feed mono f32 samples. Returns the matched keyword once a wake phrase is
    /// found. The stream is reset automatically after a detection.
    pub fn accept(&mut self, sample_rate: u32, samples: &[f32]) -> Option<String> {
        if samples.is_empty() {
            return None;
        }

        self.stream.accept_waveform(sample_rate as i32, samples);

        while self.spotter.is_ready(&self.stream) {
            self.spotter.decode(&self.stream);

            if let Some(result) = self.spotter.get_result(&self.stream) {
                if !result.keyword.is_empty() {
                    self.spotter.reset(&self.stream);
                    return Some(result.keyword);
                }
            }
        }

        None
    }

    pub fn reset(&mut self) {
        self.spotter.reset(&self.stream);
    }

    /// Offline helper used by `--wav`. Feeds the file in chunks and then adds a
    /// second of silence so the trailing blanks can be decoded.
    pub fn accept_wav(&mut self, sample_rate: i32, samples: &[f32]) -> Option<String> {
        for chunk in samples.chunks(4_800) {
            if let Some(keyword) = self.accept(sample_rate as u32, chunk) {
                return Some(keyword);
            }
        }

        let padding = vec![0.0f32; sample_rate.max(1) as usize];
        self.accept(sample_rate as u32, &padding)
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

    path.into_os_string()
        .into_string()
        .map_err(|_| format!("model path is not valid UTF-8: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_bpe_tokenized() {
        assert!(KEYWORDS.lines().count() >= 4);
        assert!(KEYWORDS.contains("▁HE Y"));
        assert!(KEYWORDS.ends_with('\n'));
    }
}
