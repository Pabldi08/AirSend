use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceiverPreferences {
    pub volume: f32,
    pub latency_ms: u32,
    pub reconnect: bool,
}
impl Default for ReceiverPreferences {
    fn default() -> Self {
        Self {
            volume: 0.2,
            latency_ms: 3000,
            reconnect: false,
        }
    }
}
impl ReceiverPreferences {
    pub fn validate(&self) -> Result<(), String> {
        if !self.volume.is_finite() || !(0.0..=1.0).contains(&self.volume) {
            return Err("invalid_volume".into());
        }
        cap_core::streaming::validate_latency_ms(self.latency_ms).map_err(|e| e.to_string())
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioOptions {
    pub device_id: Option<String>,
    pub mute_local: bool,
}

#[derive(Deserialize)]
pub struct ReceiverPreferencesPatch {
    pub volume: Option<f32>,
    pub latency_ms: Option<u32>,
    pub reconnect: Option<bool>,
}
impl ReceiverPreferencesPatch {
    pub fn apply(self, preferences: &mut ReceiverPreferences) {
        if let Some(volume) = self.volume {
            preferences.volume = volume;
        }
        if let Some(latency) = self.latency_ms {
            preferences.latency_ms = latency;
        }
        if let Some(reconnect) = self.reconnect {
            preferences.reconnect = reconnect;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_patch_preserves_latency_and_reconnect_and_rejects_invalid_values() {
        let mut preferences = ReceiverPreferences {
            volume: 0.2,
            latency_ms: 200,
            reconnect: true,
        };
        ReceiverPreferencesPatch {
            volume: Some(0.5),
            latency_ms: None,
            reconnect: None,
        }
        .apply(&mut preferences);
        assert_eq!(preferences.volume, 0.5);
        assert_eq!(preferences.latency_ms, 200);
        assert!(preferences.reconnect);
        assert!(preferences.validate().is_ok());
        for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            preferences.volume = invalid;
            assert!(preferences.validate().is_err());
        }
    }
}
