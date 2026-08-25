//! Regression coverage for cancelling a paced mock batch (Round 1 B4).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, SampleUnit, SystemClock};
use quickvib_device::{
    DeviceBackend, DeviceOpenOptions, MockBackend, StreamOutcome, StreamRequest, BATCH_SAMPLES,
};

#[test]
fn paced_stream_cancel_wakes_before_full_batch_deadline() {
    const BATCH_DURATION: Duration = Duration::from_millis(750);
    const PROMPT_CANCEL_BOUND: Duration = Duration::from_millis(250);

    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let delivered = Arc::new(AtomicUsize::new(0));
    let worker_delivered = Arc::clone(&delivered);
    let (starting_tx, starting_rx) = mpsc::sync_channel(0);
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);

    let worker = std::thread::spawn(move || {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
        let sample_rate_hz = BATCH_SAMPLES as f64 / BATCH_DURATION.as_secs_f64();
        let mut backend = MockBackend::new(clock).with_pacing(true);
        backend
            .open(&DeviceOpenOptions::new(
                sample_rate_hz,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();
        let request = StreamRequest::new(
            BATCH_SAMPLES as u64,
            SampleUnit::VelocityUmPerSec,
            sample_rate_hz,
        );

        starting_tx.send(()).unwrap();
        let result = backend.stream(
            &request,
            &mut |batch| {
                worker_delivered.fetch_add(batch.samples.len(), Ordering::SeqCst);
                Ok(())
            },
            &worker_cancel,
        );
        finished_tx.send(result).unwrap();
    });

    starting_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("paced stream worker did not start");
    // Let the worker enter the first batch's pacing wait before cancellation.
    std::thread::sleep(Duration::from_millis(50));
    cancel.cancel();

    let prompt_result = finished_rx.recv_timeout(PROMPT_CANCEL_BOUND);
    if prompt_result.is_err() {
        // Do not leave the intentionally slow baseline worker detached after the assertion.
        let _ = finished_rx.recv_timeout(Duration::from_secs(1));
    }
    worker.join().unwrap();

    let outcome = prompt_result
        .expect("paced stream observed cancellation only after the full batch delay")
        .unwrap();
    assert_eq!(outcome, StreamOutcome::Cancelled);
    assert_eq!(
        delivered.load(Ordering::SeqCst),
        0,
        "a batch was published after cancellation during pacing"
    );
}
