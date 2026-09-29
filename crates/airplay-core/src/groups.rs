//! Stereo identity comes from tight-sync metadata, never from names or IP proximity.
use crate::Device;
use std::collections::HashMap;
#[derive(Debug, Clone, serde::Serialize)]
pub struct StereoPair {
    pub id: String,
    pub name: String,
    pub member_ids: Vec<String>,
    pub complete: bool,
}

pub fn stereo_pairs(devices: &[Device]) -> Vec<StereoPair> {
    let mut groups: HashMap<String, HashMap<String, Device>> = HashMap::new();
    for device in devices
        .iter()
        .filter(|d| d.kind == crate::DeviceKind::HomePod)
    {
        if let Some(id) = &device.tight_sync_id {
            let members = groups.entry(id.clone()).or_default();
            let old = members.get(&device.receiver_id());
            if old.is_none() || device.id.contains("._airplay._tcp.") {
                members.insert(device.receiver_id(), device.clone());
            }
        }
    }
    groups
        .into_iter()
        .map(|(id, members)| {
            let mut members = members.into_values().collect::<Vec<_>>();
            members.sort_by_key(|d| (!d.is_group_leader, d.receiver_id()));
            StereoPair {
                id: format!("stereo:{id}"),
                name: members
                    .iter()
                    .find_map(|d| d.group_name.clone())
                    .unwrap_or_else(|| "HomePod".into()),
                complete: members.len() == 2 && members.iter().all(|d| d.available),
                member_ids: members.into_iter().map(|d| d.id).collect(),
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deduplicates_routes_and_does_not_mistake_a_multiroom_group_for_stereo() {
        let make = |id: &str, mac: &str, tight: Option<&str>| {
            let mut d = Device::from_txt(
                id.into(),
                "HomePod".into(),
                "speaker.local".into(),
                vec![],
                7000,
                HashMap::from([
                    ("model".into(), "AudioAccessory5,1".into()),
                    ("deviceid".into(), mac.into()),
                    ("gid".into(), "room-group".into()),
                ]),
            );
            d.tight_sync_id = tight.map(String::from);
            d
        };
        let a = make("a._airplay._tcp.local.", "001122334455", Some("pair"));
        let mut raop = a.clone();
        raop.id = "a._raop._tcp.local.".into();
        assert!(!stereo_pairs(&[a.clone(), raop.clone()])[0].complete);
        let b = make("b._airplay._tcp.local.", "001122334466", Some("pair"));
        assert!(stereo_pairs(&[a, raop, b])[0].complete);
        assert!(stereo_pairs(&[make("c", "001122334477", None)]).is_empty());
    }
}
