//! Numerical WASAPI capture probe. Does not save audio samples.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        collections::VecDeque,
        time::{Duration, Instant},
    };
    use wasapi::{Direction, SampleType, ShareMode, WaveFormat};
    wasapi::initialize_mta().ok()?;
    let args: Vec<String> = std::env::args().collect();
    let drain = args.get(1).map(String::as_str) != Some("single");
    let seconds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(15);
    // Optional scheduling delay simulates coalesced notifications after a
    // descheduled capture thread, without changing device or application audio.
    let delay_every: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let device = wasapi::get_default_device(&Direction::Render)?;
    let name = device.get_friendlyname()?;
    if !name.contains("CABLE Input") {
        return Err("Select regular CABLE Input first".into());
    }
    let mut client = device.get_iaudioclient()?;
    let format = WaveFormat::new(16, 16, &SampleType::Int, 44100, 2, None);
    let (period, _) = client.get_periods()?;
    client.initialize_client(
        &format,
        period,
        &Direction::Capture,
        &ShareMode::Shared,
        true,
    )?;
    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    client.start_stream()?;
    let start = Instant::now();
    let mut frames = 0u64;
    let mut packets = 0u64;
    let mut discontinuities = 0u64;
    let mut timestamp_errors = 0u64;
    let mut max_batch = 0u64;
    let mut data = VecDeque::new();
    let mut wakes = 0u64;
    while start.elapsed() < Duration::from_secs(seconds) {
        let _ = event.wait_for_event(100);
        wakes += 1;
        if delay_every > 0 && wakes % delay_every == 0 {
            std::thread::sleep(Duration::from_millis(15));
        }
        let mut batch = 0;
        loop {
            if capture.get_next_nbr_frames()? == Some(0) {
                break;
            }
            let flags = capture.read_from_device_to_deque(&mut data)?;
            frames += (data.len() / 4) as u64;
            data.clear();
            packets += 1;
            batch += 1;
            discontinuities += flags.data_discontinuity as u64;
            timestamp_errors += flags.timestamp_error as u64;
            if !drain {
                break;
            }
        }
        max_batch = max_batch.max(batch);
    }
    client.stop_stream()?;
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "{}",
        serde_json::json!({"mode":if drain {"drain"} else {"single"},"seconds":elapsed,"sample_frames":frames,"sample_frames_per_second":frames as f64/elapsed,"packets":packets,"discontinuities":discontinuities,"timestamp_errors":timestamp_errors,"max_packets_per_wake":max_batch,"scheduling_delay_every_wakes":delay_every})
    );
    wasapi::deinitialize();
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}
