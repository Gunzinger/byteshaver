//! GUI-side [`Reporter`] implementation: forwards every
//! [`JobEvent`][byteshaver::job::JobEvent] of a running job into a
//! `std::sync::mpsc` channel that the UI thread drains once per frame
//! (plan WS8 §5.2).
//!
//! The reporter itself lives on the pipeline's rayon workers, so it must be
//! `Send + Sync` (the [`Reporter`] supertraits); the `Sender` is guarded by
//! a mutex to be independent of the std channel's `Sync` status.

use std::sync::Mutex;
use std::sync::mpsc::Sender;

use byteshaver::job::{JobEvent, Reporter};

/// Sends every event into the UI channel; send failures (UI thread gone)
/// are silently dropped — a shutting-down GUI no longer needs them.
pub struct ChannelReporter {
    tx: Mutex<Sender<JobEvent>>,
}

impl ChannelReporter {
    /// Wraps the UI-side receiver end of `tx`.
    #[must_use]
    pub fn new(tx: Sender<JobEvent>) -> Self {
        ChannelReporter { tx: Mutex::new(tx) }
    }
}

impl Reporter for ChannelReporter {
    fn on_event(&self, ev: JobEvent) {
        // a broken pipe only means the window closed mid-run
        let _ = self
            .tx
            .lock()
            .expect("channel reporter mutex poisoned")
            .send(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn forwards_events_into_the_channel() {
        let (tx, rx) = mpsc::channel::<JobEvent>();
        let reporter = ChannelReporter::new(tx);
        reporter.on_event(JobEvent::Started { total_files: 2 });
        reporter.on_event(JobEvent::Finished);
        assert!(matches!(
            rx.try_recv(),
            Ok(JobEvent::Started { total_files: 2 })
        ));
        assert!(matches!(rx.try_recv(), Ok(JobEvent::Finished)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn is_send_and_sync_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ChannelReporter>();
    }
}
