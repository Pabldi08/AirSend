//! Direct AirPlay tone with an absolute producer clock; bypasses Windows audio.
use cap_core::streaming::open_live_stream;
use std::{net::IpAddr, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info,mdns_sd=off")
        .init();
    let args: Vec<String> = std::env::args().collect();
    let ip: IpAddr = args.get(1).ok_or("IP required")?.parse()?;
    let seconds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(45);
    let peers = cap_core::discovery::browse_once(Duration::from_secs(5)).await?;
    let device = peers
        .iter()
        .find(|d| d.addresses.contains(&ip) && d.id.contains("_airplay._tcp"))
        .ok_or("HomePod advertisement not found")?;
    let descriptor = device.descriptor(ip, &peers)?;
    let handle = open_live_stream(descriptor, Some(0.26), Some(300)).await?;
    println!("direct_tone_connected_and_starting");
    let rate = handle.sample_rate() as u64;
    let channels = handle.channels() as usize;
    let start = tokio::time::Instant::now();
    let total = rate * seconds;
    let mut frame = 0u64;
    let mut rejected = 0u64;
    while frame < total {
        let count = (total - frame).min(352);
        let mut pcm = Vec::with_capacity(count as usize * channels);
        for i in 0..count {
            let s = (((frame + i) as f64 * std::f64::consts::TAU * 997.0 / rate as f64).sin()
                * 0.12
                * i16::MAX as f64) as i16;
            pcm.extend(std::iter::repeat(s).take(channels));
        }
        if !handle.push_pcm(pcm) {
            rejected += 1;
        }
        frame += count;
        tokio::time::sleep_until(start + Duration::from_secs_f64(frame as f64 / rate as f64)).await;
    }
    println!(
        "test=direct_airplay_tone seconds={:.3} sample_frames={} rejected_blocks={}",
        start.elapsed().as_secs_f64(),
        frame,
        rejected
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    drop(handle);
    Ok(())
}
