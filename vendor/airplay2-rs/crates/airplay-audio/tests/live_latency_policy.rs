use airplay_audio::{AudioStreamer, LiveAudioDecoder, LivePcmFrame, LiveStreamOptions};
use airplay_core::StreamConfig;
use std::time::{Duration, Instant};

fn block(value: i16) -> LivePcmFrame {
    LivePcmFrame {
        samples: vec![value; 704],
        channels: 2,
        sample_rate: 44_100,
    }
}

#[test]
fn overloaded_input_retains_recent_pcm_and_reports_evictions() {
    let (sender, mut decoder) =
        LiveAudioDecoder::create_pair_with_max_age(44_100, 2, 7, Some(Duration::from_millis(120)));
    for i in 0..100 {
        assert!(sender.try_send(block(i)));
    }
    assert_eq!(decoder.decode_frame().unwrap().unwrap().samples[0], 93);
    assert_eq!(sender.diagnostics().queue_drops, 93);
    drop(decoder);
    assert!(sender.is_closed());
    assert!(!sender.try_send(block(101)));
}

#[test]
fn stable_mode_keeps_newest_drop_behavior() {
    let (sender, mut decoder) = LiveAudioDecoder::create_pair(44_100, 2, 2);
    assert!(sender.try_send(block(1)));
    assert!(sender.try_send(block(2)));
    assert!(!sender.try_send(block(3)));
    assert_eq!(decoder.decode_frame().unwrap().unwrap().samples[0], 1);
}

#[test]
fn consumer_stall_expires_input_instead_of_replaying_it() {
    let (sender, mut decoder) =
        LiveAudioDecoder::create_pair_with_max_age(44_100, 2, 7, Some(Duration::from_millis(5)));
    assert!(sender.try_send(block(42)));
    std::thread::sleep(Duration::from_millis(20));
    assert!(decoder.decode_frame().unwrap().is_none());
    assert_eq!(sender.diagnostics().stale_drops, 1);
    assert!(sender.try_send(block(99)));
    assert_eq!(decoder.decode_frame().unwrap().unwrap().samples[0], 99);
}

#[tokio::test]
async fn feeding_before_start_makes_low_latency_preroll_ready_without_timeout() {
    let (sender, decoder) =
        LiveAudioDecoder::create_pair_with_max_age(44_100, 2, 7, Some(Duration::from_millis(120)));
    for _ in 0..5 {
        assert!(sender.try_send(block(1)));
    }
    let mut streamer = AudioStreamer::new(StreamConfig::default());
    let began = Instant::now();
    streamer
        .start_live_with_options(decoder, LiveStreamOptions::low_latency())
        .await
        .unwrap();
    assert!(
        began.elapsed() < Duration::from_millis(300),
        "pre-roll must not exhaust the 500 ms timeout"
    );
    streamer.stop().await.unwrap();
}

#[tokio::test]
async fn stable_preroll_can_be_fed_during_start_instead_of_waiting_five_seconds() {
    let (sender, decoder) = LiveAudioDecoder::create_pair(44_100, 2, 140);
    for _ in 0..130 {
        assert!(sender.try_send(block(1)));
    }
    let mut streamer = AudioStreamer::new(StreamConfig::default());
    let began = Instant::now();
    streamer.start_live(decoder).await.unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "producer must feed pre-roll before readiness is awaited"
    );
    streamer.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_invalid_local_policy_without_starting() {
    let (_, decoder) = LiveAudioDecoder::create_pair(44_100, 2, 7);
    let mut streamer = AudioStreamer::new(StreamConfig::default());
    let mut options = LiveStreamOptions::low_latency();
    options.sender_capacity = 0;
    assert!(streamer
        .start_live_with_options(decoder, options)
        .await
        .is_err());
}

#[test]
fn discarded_input_advances_media_position_without_rewinding() {
    let (sender, mut decoder) =
        LiveAudioDecoder::create_pair_with_max_age(44_100, 2, 7, Some(Duration::from_millis(5)));
    assert!(sender.try_send(block(1)));
    std::thread::sleep(Duration::from_millis(20));
    assert!(decoder.decode_frame().unwrap().is_none());
    assert!(sender.try_send(block(2)));
    let frame = decoder
        .decode_resampled(&StreamConfig::default().audio_format, 352)
        .unwrap()
        .unwrap();
    assert_eq!(frame.timestamp, 352);
}

#[test]
fn discarding_a_serialized_packet_does_not_reset_sequence_or_encryption_nonce() {
    use airplay_audio::{cipher::ChaChaPacketCipher, RtpPacket, RtpSender};
    use airplay_crypto::chacha::AudioCipher;
    let mut sender = RtpSender::new("127.0.0.1:9".parse().unwrap(), 42);
    sender.set_cipher(Box::new(ChaChaPacketCipher::new(AudioCipher::new([7; 32]))));
    let first =
        RtpPacket::parse_encrypted(&sender.prepare_audio(96, 0, &[1; 32], true).unwrap()).unwrap();
    let discarded =
        RtpPacket::parse_encrypted(&sender.prepare_audio(96, 352, &[2; 32], false).unwrap())
            .unwrap();
    let next = RtpPacket::parse_encrypted(&sender.prepare_audio(96, 704, &[3; 32], false).unwrap())
        .unwrap();
    assert_eq!(next.header.sequence, first.header.sequence.wrapping_add(2));
    assert_eq!(next.header.timestamp, 704);
    assert_ne!(next.nonce, first.nonce);
    assert_ne!(next.nonce, discarded.nonce);
}

#[test]
fn diagnostic_handle_does_not_keep_the_pcm_producer_alive() {
    let (sender, mut decoder) = LiveAudioDecoder::create_pair(44_100, 2, 7);
    let diagnostics = sender.diagnostics_handle();
    drop(sender);
    assert!(decoder.decode_frame().unwrap().is_none());
    assert!(decoder.is_eof());
    assert_eq!(diagnostics.snapshot().queued_blocks, 0);
}

#[test]
fn diagnostic_handle_does_not_retain_pcm_after_decoder_shutdown() {
    let (sender, decoder) = LiveAudioDecoder::create_pair(44_100, 2, 7);
    let diagnostics = sender.diagnostics_handle();
    assert!(sender.try_send(block(1)));
    drop(decoder);
    assert!(sender.is_closed());
    assert_eq!(diagnostics.snapshot().queued_blocks, 0);
}

#[tokio::test]
async fn dropping_owner_marks_control_clone_finished() {
    let (sender, decoder) =
        LiveAudioDecoder::create_pair_with_max_age(44_100, 2, 7, Some(Duration::from_millis(120)));
    for _ in 0..5 {
        assert!(sender.try_send(block(1)));
    }
    let mut streamer = AudioStreamer::new(StreamConfig::default());
    streamer
        .start_live_with_options(decoder, LiveStreamOptions::low_latency())
        .await
        .unwrap();
    let control = streamer.clone();
    drop(streamer);
    assert!(control.is_finished());
    drop(control);
    tokio::task::yield_now().await;
    assert!(sender.is_closed());
}

#[tokio::test]
async fn unprimed_handle_api_can_start_without_waiting_for_an_unavailable_producer() {
    let (_, decoder) = LiveAudioDecoder::create_pair(44_100, 2, 64);
    let mut streamer = AudioStreamer::new(StreamConfig::default());
    let mut options = LiveStreamOptions::default();
    options.startup_ms = 0;
    let began = Instant::now();
    streamer
        .start_live_with_options(decoder, options)
        .await
        .unwrap();
    assert!(began.elapsed() < Duration::from_millis(200));
    streamer.stop().await.unwrap();
}
