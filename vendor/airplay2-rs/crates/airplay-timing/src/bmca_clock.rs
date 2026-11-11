//! Correlates PTP exchanges without mixing timestamps from different packets.
use crate::ClockOffset;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Peer {
    pub port: [u8; 10],
    pub domain: u8,
}
#[derive(Clone, Copy)]
pub(crate) struct SyncPair {
    peer: Peer,
    t1: u128,
    t2: u128,
}
struct Request {
    pair: SyncPair,
    sequence: u16,
    requester: [u8; 10],
    t3: u128,
    sent: Instant,
}
#[derive(Default)]
pub(crate) struct BmcaClock {
    sync: Option<(Peer, u16, u128)>,
    request: Option<Request>,
    samples: VecDeque<i128>,
    last_offset: Option<i64>,
    two_way_at: Option<Instant>,
}
impl BmcaClock {
    pub fn sync(&mut self, peer: Peer, sequence: u16, received_ns: u128) {
        self.sync = Some((peer, sequence, received_ns));
    }
    pub fn follow_up(&mut self, peer: Peer, sequence: u16, origin_ns: u128) -> Option<SyncPair> {
        let (source, seq, received) = self.sync?;
        if peer != source || sequence != seq {
            return None;
        }
        self.sync = None;
        Some(SyncPair {
            peer,
            t1: origin_ns,
            t2: received,
        })
    }
    pub fn one_way(&mut self, pair: SyncPair, now: Instant) -> Option<ClockOffset> {
        let sample = i128::try_from(pair.t1).ok()? - i128::try_from(pair.t2).ok()?;
        i64::try_from(sample).ok()?;
        if self.samples.len() == 32 {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        // A lost Delay_Resp must not disable Sync/Follow_Up tracking forever.
        if self
            .two_way_at
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(3))
        {
            return None;
        }
        let best = *self.samples.iter().max()?;
        let spread = best - *self.samples.iter().min()?;
        let offset_ns = i64::try_from(best).ok()?;
        if self
            .last_offset
            .is_some_and(|prev| (i128::from(prev) - best).abs() <= 500_000)
        {
            return None;
        }
        self.last_offset = Some(offset_ns);
        Some(ClockOffset {
            offset_ns,
            error_ns: (spread / 2) as u64,
            rtt_ns: 0,
        })
        // error_ns describes observed jitter, not unknown one-way path bias.
    }
    pub fn request(
        &mut self,
        pair: SyncPair,
        sequence: u16,
        requester: [u8; 10],
        t3: u128,
        now: Instant,
    ) {
        self.request = Some(Request {
            pair,
            sequence,
            requester,
            t3,
            sent: now,
        });
    }
    pub fn response(
        &mut self,
        peer: Peer,
        sequence: u16,
        requester: [u8; 10],
        t4: u128,
        now: Instant,
    ) -> Option<ClockOffset> {
        let r = self.request.as_ref()?;
        if peer != r.pair.peer
            || sequence != r.sequence
            || requester != r.requester
            || now.saturating_duration_since(r.sent) > Duration::from_secs(2)
        {
            return None;
        }
        let r = self.request.take()?;
        let forward = i128::try_from(r.pair.t2).ok()? - i128::try_from(r.pair.t1).ok()?;
        let reverse = i128::try_from(t4).ok()? - i128::try_from(r.t3).ok()?;
        let delay = (forward + reverse) / 2;
        if !(0..=500_000_000).contains(&delay) {
            return None;
        }
        let offset_ns = i64::try_from((reverse - forward) / 2).ok()?;
        self.two_way_at = Some(now);
        self.last_offset = Some(offset_ns);
        Some(ClockOffset {
            offset_ns,
            error_ns: delay as u64,
            rtt_ns: delay as u64 * 2,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const MASTER: Peer = Peer {
        port: [1; 10],
        domain: 0,
    };
    fn pair(c: &mut BmcaClock, t1: u128, t2: u128) -> SyncPair {
        c.sync(MASTER, u16::MAX, t2);
        c.follow_up(MASTER, u16::MAX, t1).unwrap()
    }
    #[test]
    fn rejects_unmatched_followups_and_duplicates() {
        let mut c = BmcaClock::default();
        c.sync(MASTER, 0, 100);
        assert!(c.follow_up(MASTER, 1, 200).is_none());
        assert!(c
            .follow_up(
                Peer {
                    domain: 1,
                    ..MASTER
                },
                0,
                200
            )
            .is_none());
        assert!(c
            .follow_up(
                Peer {
                    port: [2; 10],
                    ..MASTER
                },
                0,
                200
            )
            .is_none());
        assert!(c.follow_up(MASTER, 0, 200).is_some());
        assert!(c.follow_up(MASTER, 0, 200).is_none());
    }
    #[test]
    fn one_way_tracks_uptime_clock_and_uses_least_delayed_sample() {
        let mut c = BmcaClock::default();
        let now = Instant::now();
        let p = pair(&mut c, 4_000_000, 1_700_000_000_000_000_000);
        let first = c.one_way(p, now).unwrap();
        assert!(first.offset_ns < 0);
        let p = pair(&mut c, 5_000_000, 1_700_000_000_000_000_000);
        assert_eq!(
            c.one_way(p, now).unwrap().offset_ns,
            first.offset_ns + 1_000_000
        );
        let p = pair(&mut c, 1, 1_700_000_000_000_000_000);
        assert!(c.one_way(p, now).is_none());
    }
    #[test]
    fn two_way_validates_sequence_identity_domain_and_frozen_pair() {
        let mut c = BmcaClock::default();
        let now = Instant::now();
        let p = pair(&mut c, 10000, 9010);
        c.request(p, 7, [3; 10], 10000, now);
        // A later Sync must not change the timestamps of the outstanding request.
        c.sync(MASTER, 1, 999999);
        assert!(c.response(MASTER, 6, [3; 10], 11010, now).is_none());
        assert!(c.response(MASTER, 7, [4; 10], 11010, now).is_none());
        assert!(c
            .response(
                Peer {
                    domain: 1,
                    ..MASTER
                },
                7,
                [3; 10],
                11010,
                now
            )
            .is_none());
        let offset = c.response(MASTER, 7, [3; 10], 11010, now).unwrap();
        assert_eq!(offset.offset_ns, 1000);
        assert_eq!(offset.rtt_ns, 20);
        assert!(c.response(MASTER, 7, [3; 10], 11010, now).is_none());
    }
    #[test]
    fn two_way_sign_and_timeout_fallback() {
        let mut c = BmcaClock::default();
        let now = Instant::now();
        let p = pair(&mut c, 9000, 10010);
        c.request(p, 1, [3; 10], 11000, now);
        assert_eq!(
            c.response(MASTER, 1, [3; 10], 10010, now)
                .unwrap()
                .offset_ns,
            -1000
        );
        let p = pair(&mut c, 2_000_000, 1_000_000);
        assert!(c.one_way(p, now + Duration::from_secs(1)).is_none());
        assert!(c.one_way(p, now + Duration::from_secs(4)).is_some());
    }
    #[test]
    fn stale_response_cannot_disable_one_way_estimation() {
        let mut c = BmcaClock::default();
        let now = Instant::now();
        let p = pair(&mut c, 10000, 9010);
        c.request(p, 1, [3; 10], 10000, now);
        assert!(c
            .response(MASTER, 1, [3; 10], 11010, now + Duration::from_secs(3))
            .is_none());
        assert!(c.one_way(p, now + Duration::from_secs(3)).is_some());
    }
}
