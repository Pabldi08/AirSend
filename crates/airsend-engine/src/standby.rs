use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Options {
    pub enabled: bool,
    pub minutes: u32,
    pub threshold_dbfs: i32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: false,
            minutes: 5,
            threshold_dbfs: -65,
        }
    }
}

impl Options {
    pub fn validate(self) -> Result<(), String> {
        if !(1..=60).contains(&self.minutes) || !(-90..=-40).contains(&self.threshold_dbfs) {
            return Err("invalid_standby_options".into());
        }
        Ok(())
    }
}

pub(crate) struct SilenceDetector {
    threshold: f64,
    timeout: Duration,
    since: Option<Instant>,
}

impl SilenceDetector {
    pub fn reset(&mut self) {
        self.since = None;
    }

    pub fn new(options: Options) -> Self {
        Self {
            threshold: 32768.0 * 10.0_f64.powf(f64::from(options.threshold_dbfs) / 20.0),
            timeout: Duration::from_secs(u64::from(options.minutes) * 60),
            since: None,
        }
    }

    pub fn observe(&mut self, samples: &[i16], channels: u8, now: Instant) -> bool {
        // A single quiet channel must not mask a louder one. Keep the peak
        // threshold 6 dB above the RMS threshold to catch brief audio.
        let peak_limit = if self.since.is_some() {
            self.threshold * 2.0
        } else {
            self.threshold
        };
        if channels == 0
            || samples.len() < usize::from(channels)
            || samples
                .iter()
                .any(|sample| f64::from(*sample).abs() >= peak_limit)
        {
            self.since = None;
            return false;
        }
        let channels = usize::from(channels);
        let frames = samples.len() / channels;
        let power_limit = self.threshold * self.threshold * frames as f64;
        let audible_channel = (0..channels).any(|channel| {
            samples
                .iter()
                .skip(channel)
                .step_by(channels)
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>()
                >= power_limit
        });
        if audible_channel {
            self.since = None;
            return false;
        }
        now.saturating_duration_since(*self.since.get_or_insert(now)) >= self.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standby_requires_continuous_silence_and_respects_quiet_audio_on_one_channel() {
        let start = Instant::now();
        let mut detector = SilenceDetector::new(Options {
            minutes: 1,
            ..Options::default()
        });
        assert!(!detector.observe(&[0, 0], 2, start));
        assert!(!detector.observe(&[0, 0], 2, start + Duration::from_secs(59)));
        assert!(!detector.observe(&[20, 0, 20, 0], 2, start + Duration::from_secs(59)));
        assert!(!detector.observe(&[0, 0], 2, start + Duration::from_secs(60)));
        assert!(detector.observe(&[0, 0], 2, start + Duration::from_secs(120)));
        assert!(!detector.observe(&[100, 0], 2, start + Duration::from_secs(121)));
    }

    #[test]
    fn old_settings_leave_standby_disabled() {
        let options: Options = serde_json::from_str("{}").unwrap();
        assert!(!options.enabled);
        assert_eq!(options.minutes, 5);
        assert!(options.validate().is_ok());
        assert!(Options {
            minutes: 0,
            ..options
        }
        .validate()
        .is_err());
    }
}
