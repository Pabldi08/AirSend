//! Read only receiver capability summary; never starts or changes playback.
use std::{net::IpAddr, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ip: IpAddr = std::env::args().nth(1).ok_or("IP required")?.parse()?;
    let peers = cap_core::browse_once(Duration::from_secs(4)).await?;
    let device = peers
        .iter()
        .find(|d| d.addresses.contains(&ip) && d.id.contains("_airplay._tcp"))
        .ok_or("AirPlay advertisement not found")?;
    for key in ["model", "osvers", "srcvers", "protovers", "features", "fex"] {
        println!(
            "{key}={}",
            device
                .txt
                .get(key)
                .map(String::as_str)
                .unwrap_or("(not advertised)")
        );
    }
    Ok(())
}
