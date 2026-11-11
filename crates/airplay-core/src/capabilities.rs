//! Receiver advertisements describe capabilities, not transport implementations.
//! Bit 96 in `fex` is associated with the newer macOS audio engine by the
//! primary reverse-engineering project ksha23/preroll (pair-check.py). It is
//! deliberately exposed as a hint, never as proof that a sender can use it.
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtendedFeatureStatus {
    #[default]
    Missing,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReceiverCapabilities {
    pub buffered_audio: bool,
    pub ptp: bool,
    pub responsive_audio_advertised: bool,
    pub extended_features_status: ExtendedFeatureStatus,
}

impl ReceiverCapabilities {
    pub fn from_txt(features: Option<&str>, extended: Option<&str>) -> Self {
        let legacy = features.and_then(|s| ap2rs_core::features::Features::from_txt_value(s).ok());
        let mut result = Self {
            buffered_audio: legacy.is_some_and(|f| f.supports_buffered_audio()),
            ptp: legacy.is_some_and(|f| f.supports_ptp()),
            ..Self::default()
        };
        let Some(encoded) = extended else {
            return result;
        };
        // Bound allocations independently of the discovery implementation.
        // Unknown higher bits remain accepted; only known bits are interpreted.
        if encoded.is_empty() || encoded.len() > 344 {
            result.extended_features_status = ExtendedFeatureStatus::Invalid;
            return result;
        }
        let decoded = STANDARD
            .decode(encoded)
            .or_else(|_| STANDARD_NO_PAD.decode(encoded));
        match decoded {
            Ok(bytes) if !bytes.is_empty() && bytes.len() <= 256 => {
                result.extended_features_status = ExtendedFeatureStatus::Valid;
                result.responsive_audio_advertised = bytes.get(12).is_some_and(|b| b & 1 != 0);
            }
            _ => result.extended_features_status = ExtendedFeatureStatus::Invalid,
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bit_96_above_the_legacy_mask_with_or_without_padding() {
        let mut bytes = [0u8; 16];
        bytes[12] = 1;
        for encoded in [STANDARD.encode(bytes), STANDARD_NO_PAD.encode(bytes)] {
            let caps = ReceiverCapabilities::from_txt(Some("0x0,0x300"), Some(&encoded));
            assert!(caps.responsive_audio_advertised);
            assert!(caps.buffered_audio && caps.ptp);
            assert_eq!(caps.extended_features_status, ExtendedFeatureStatus::Valid);
        }
    }

    #[test]
    fn does_not_confuse_adjacent_or_future_bits_with_responsive_audio() {
        let mut bytes = [0u8; 24];
        bytes[11] = 0x80; // bit 95
        bytes[12] = 2; // bit 97
        bytes[23] = 0x80; // unknown future bit
        let caps = ReceiverCapabilities::from_txt(None, Some(&STANDARD.encode(bytes)));
        assert!(!caps.responsive_audio_advertised);
        assert_eq!(caps.extended_features_status, ExtendedFeatureStatus::Valid);
    }

    #[test]
    fn malformed_optional_features_preserve_legacy_capabilities() {
        for value in ["", "invalid!", "A", &"A".repeat(345)] {
            let caps = ReceiverCapabilities::from_txt(Some("0x0,0x300"), Some(value));
            assert!(!caps.responsive_audio_advertised);
            assert!(caps.buffered_audio && caps.ptp);
            assert_eq!(
                caps.extended_features_status,
                ExtendedFeatureStatus::Invalid
            );
        }
    }

    #[test]
    fn missing_or_short_extended_mask_does_not_imply_new_transport_support() {
        assert_eq!(
            ReceiverCapabilities::from_txt(None, None),
            ReceiverCapabilities::default()
        );
        let caps = ReceiverCapabilities::from_txt(None, Some(&STANDARD.encode([255u8; 8])));
        assert!(!caps.responsive_audio_advertised);
        assert_eq!(caps.extended_features_status, ExtendedFeatureStatus::Valid);
    }
}
