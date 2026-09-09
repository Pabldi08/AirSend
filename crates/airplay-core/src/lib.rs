pub mod discovery;
pub mod pairing;
pub mod probe;
pub mod streaming;

pub use discovery::{browse_once, Device, DeviceKind, Discovery, DiscoveryError};
pub use pairing::{pair_homepod, DeviceDescriptor, PairedSession, PairingError};
pub use probe::{
    parse_manual_endpoint, probe_airplay, ManualEndpointError, ProbeError, ProbeResult,
};
pub use streaming::{
    open_live_stream, play_test_tone, LatencyProfile, StreamError, StreamHandle, StreamStage,
};
