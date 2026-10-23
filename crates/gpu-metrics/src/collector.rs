//! Runs NVML reads on one dedicated thread and bounds how long a scrape
//! waits for them.
//!
//! An NVML call can block for a long time (or forever) on a GPU in a bad
//! state. The HTTP side never calls NVML: it hands the collector thread a
//! reply slot and waits at most the per-scrape deadline. A late or stuck
//! collector yields a status-only response (`gpu_metrics_collect_timeout 1`)
//! instead of a hung connection, and while the collector is still busy with
//! an earlier request further scrapes fail fast rather than queueing.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread;
use std::time::Duration;

use crate::metrics::Snapshot;

/// Something that can read every GPU once.
pub trait Collect: Send + 'static {
    /// Called once on the collector thread before the first scrape.
    fn start(&mut self) {}
    fn collect(&mut self) -> Snapshot;
}

pub struct Collector {
    requests: SyncSender<SyncSender<Snapshot>>,
    deadline: Duration,
}

impl Collector {
    pub fn spawn<C: Collect>(mut backend: C, deadline: Duration) -> std::io::Result<Collector> {
        // One slot: a scrape may queue behind the one in progress; any more
        // concurrent scrapes are answered "timed out" immediately.
        let (requests, inbox): (_, Receiver<SyncSender<Snapshot>>) = sync_channel(1);
        thread::Builder::new()
            .name("collector".into())
            .spawn(move || {
                backend.start();
                for reply in inbox {
                    // The scrape may have given up already; that is fine.
                    let _ = reply.try_send(backend.collect());
                }
            })?;
        Ok(Collector { requests, deadline })
    }

    /// One scrape's snapshot, or [`Snapshot::timed_out`] if the collector
    /// does not answer within the deadline.
    pub fn scrape(&self) -> Snapshot {
        let (reply, answer) = sync_channel(1);
        if self.requests.try_send(reply).is_err() {
            return Snapshot::timed_out();
        }
        answer
            .recv_timeout(self.deadline)
            .unwrap_or_else(|_| Snapshot::timed_out())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    struct Fake {
        delay: Duration,
        calls: Arc<AtomicU32>,
    }

    impl Collect for Fake {
        fn collect(&mut self) -> Snapshot {
            thread::sleep(self.delay);
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Snapshot {
                nvml_up: true,
                devices: n,
                ..Snapshot::down()
            }
        }
    }

    #[test]
    fn answers_within_deadline() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = Collector::spawn(
            Fake {
                delay: Duration::ZERO,
                calls: calls.clone(),
            },
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(c.scrape().devices, 1);
        assert_eq!(c.scrape().devices, 2);
    }

    #[test]
    fn stuck_collector_never_hangs_a_scrape() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = Collector::spawn(
            Fake {
                delay: Duration::from_secs(3600),
                calls,
            },
            Duration::from_millis(100),
        )
        .unwrap();
        let start = Instant::now();
        // First waits out the deadline; the second queues behind it and also
        // times out; the rest find the queue full and return at once.
        for _ in 0..5 {
            let s = c.scrape();
            assert!(s.timed_out);
            assert!(!s.nvml_up);
        }
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
