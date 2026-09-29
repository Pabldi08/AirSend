//! Cancellation tokens for session operations and stale-event filtering.
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;

pub struct SessionControl {
    generation: AtomicU64,
    changes: watch::Sender<u64>,
}
impl Default for SessionControl {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            generation: AtomicU64::new(0),
            changes,
        }
    }
}
impl SessionControl {
    pub fn begin(&self) -> SessionToken {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        // Concurrent commands must never publish an older cancellation generation.
        self.changes.send_if_modified(|current| {
            if generation > *current {
                *current = generation;
                true
            } else {
                false
            }
        });
        SessionToken {
            generation,
            changes: self.changes.subscribe(),
        }
    }
    pub fn subscribe(&self, generation: u64) -> SessionToken {
        SessionToken {
            generation,
            changes: self.changes.subscribe(),
        }
    }
    pub fn cancel(&self) {
        self.begin();
    }
    pub fn current(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}
pub struct SessionToken {
    pub generation: u64,
    changes: watch::Receiver<u64>,
}
impl SessionToken {
    pub async fn cancelled(&mut self) {
        loop {
            if *self.changes.borrow_and_update() != self.generation {
                return;
            }
            if self.changes.changed().await.is_err() {
                return;
            }
        }
    }
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionEvent {
    pub generation: u64,
    pub code: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stopping_cancels_preparation_without_waiting_for_network() {
        let control = SessionControl::default();
        let mut token = control.begin();
        control.cancel();
        tokio::time::timeout(std::time::Duration::from_millis(50), token.cancelled())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn replacement_invalidates_old_events_and_keeps_new_token_active() {
        let control = SessionControl::default();
        let mut old = control.begin();
        let mut new = control.begin();
        old.cancelled().await;
        assert_ne!(old.generation, control.current());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), new.cancelled())
                .await
                .is_err()
        );
    }
}

/// Only known transient transport/capture failures merit automatic retry.
pub fn recoverable_failure(message: &str) -> bool {
    let message = message.to_lowercase();
    [
        "timeout",
        "timed out",
        "connection refused",
        "connection reset",
        "broken pipe",
        "network",
        "unavailable",
        "not connected",
        "io error",
    ]
    .iter()
    .any(|text| message.contains(text))
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn configuration_and_authentication_failures_do_not_repeat() {
        for message in [
            "invalid SRP proof",
            "wrong password",
            "receiver does not advertise PTP",
            "unsupported format",
            "status 403",
        ] {
            assert!(!recoverable_failure(message));
        }
        for message in [
            "pairing timeout (15s)",
            "RTSP connection refused",
            "selected_audio_output_unavailable",
        ] {
            assert!(recoverable_failure(message));
        }
    }
    #[test]
    fn concurrent_cancellation_never_publishes_an_older_generation() {
        let control = std::sync::Arc::new(SessionControl::default());
        let workers = (0..32)
            .map(|_| {
                let control = control.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        control.begin();
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(control.current(), 3200);
        assert_eq!(*control.changes.borrow(), control.current());
    }
}
