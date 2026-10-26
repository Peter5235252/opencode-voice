use std::borrow::Cow;
use std::collections::VecDeque;

/// Result of filtering one audio chunk through the wake-word energy gate.
pub enum GateAction<'a> {
    /// Quiet audio was dropped.
    Skip,
    /// The stream has been quiet long enough to reset the wake detector.
    Deactivate,
    /// Audio should be fed to the wake detector.
    Feed(Cow<'a, [f32]>),
}

/// A tiny RMS gate in front of the always-on keyword spotter. It keeps a short
/// preroll so the beginning of a word is not clipped, and keeps decoding for a
/// second of silence so pauses inside a phrase do not split it.
pub struct WakeGate {
    threshold: f32,
    preroll: VecDeque<f32>,
    preroll_capacity: usize,
    quiet_samples: usize,
    quiet_limit: usize,
    active: bool,
}

impl WakeGate {
    pub fn new(sample_rate: u32, threshold: f32) -> Self {
        let sample_rate = sample_rate.max(8_000) as usize;

        Self {
            threshold,
            preroll: VecDeque::with_capacity(sample_rate / 2),
            preroll_capacity: sample_rate / 2,
            quiet_samples: 0,
            quiet_limit: sample_rate,
            active: false,
        }
    }

    pub fn filter<'a>(&mut self, samples: &'a [f32]) -> GateAction<'a> {
        if samples.is_empty() {
            return GateAction::Skip;
        }

        if self.threshold <= 0.0 {
            return GateAction::Feed(Cow::Borrowed(samples));
        }

        let rms = rms(samples);

        if !self.active {
            if rms < self.threshold {
                self.push_preroll(samples);
                return GateAction::Skip;
            }

            self.active = true;
            self.quiet_samples = 0;

            let mut audio = Vec::with_capacity(self.preroll.len() + samples.len());
            audio.extend(self.preroll.drain(..));
            audio.extend_from_slice(samples);
            return GateAction::Feed(Cow::Owned(audio));
        }

        if rms >= self.threshold {
            self.quiet_samples = 0;
        } else {
            self.quiet_samples += samples.len();
        }

        if self.quiet_samples >= self.quiet_limit {
            self.reset();
            return GateAction::Deactivate;
        }

        GateAction::Feed(Cow::Borrowed(samples))
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.quiet_samples = 0;
        self.preroll.clear();
    }

    fn push_preroll(&mut self, samples: &[f32]) {
        self.preroll.extend(samples.iter().copied());
        while self.preroll.len() > self.preroll_capacity {
            self.preroll.pop_front();
        }
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action_is_feed(action: &GateAction<'_>) -> bool {
        matches!(action, GateAction::Feed(_))
    }

    #[test]
    fn quiet_audio_is_skipped() {
        let mut gate = WakeGate::new(16_000, 0.01);
        assert!(matches!(gate.filter(&[0.0; 1_600]), GateAction::Skip));
    }

    #[test]
    fn speech_preroll_is_replayed() {
        let mut gate = WakeGate::new(16_000, 0.01);
        assert!(matches!(gate.filter(&[0.0; 1_600]), GateAction::Skip));

        let action = gate.filter(&[0.5; 1_600]);
        let GateAction::Feed(audio) = action else {
            panic!("expected audio to be fed");
        };
        assert_eq!(audio.len(), 3_200);
        assert_eq!(audio[0], 0.0);
        assert_eq!(audio[3_199], 0.5);
    }

    #[test]
    fn silence_after_speech_deactivates() {
        let mut gate = WakeGate::new(16_000, 0.01);
        assert!(action_is_feed(&gate.filter(&[0.5; 1_600])));

        for _ in 0..9 {
            assert!(action_is_feed(&gate.filter(&[0.0; 1_600])));
        }

        assert!(matches!(gate.filter(&[0.0; 1_600]), GateAction::Deactivate));
        assert!(matches!(gate.filter(&[0.0; 1_600]), GateAction::Skip));
    }

    #[test]
    fn zero_threshold_disables_the_gate() {
        let mut gate = WakeGate::new(16_000, 0.0);
        assert!(action_is_feed(&gate.filter(&[0.0; 1_600])));
    }
}
