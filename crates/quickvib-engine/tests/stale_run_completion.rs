//! Public-API coverage for a stale reader finishing after a newer sequence (Round 1 B1).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{mpsc, Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, NullLogger, SampleUnit, ScpiError, TestClock};
use quickvib_device::{
    DeviceBackend, DeviceCapabilities, DeviceError, DeviceOpenOptions, SampleBatch, StreamOutcome,
    StreamRequest,
};
use quickvib_engine::{Engine, EngineConfig, NotificationSink, State};
use quickvib_project::Project;

const PROJECT: &str = r#"{
    "schemaVersion": 1,
    "name": "SequenceGuard",
    "device": { "sampleRateHz": 10.0, "unit": "velocity_um_s" },
    "recording": { "durationSeconds": 0.1 }
}"#;

struct DeferredBackend {
    open: bool,
    entered: Option<mpsc::SyncSender<()>>,
    returning: Option<mpsc::SyncSender<()>>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl DeviceBackend for DeferredBackend {
    fn is_connected(&self) -> bool {
        self.open
    }

    fn open(&mut self, _options: &DeviceOpenOptions) -> Result<(), DeviceError> {
        self.open = true;
        Ok(())
    }

    fn capabilities(&self) -> Result<DeviceCapabilities, DeviceError> {
        if !self.open {
            return Err(DeviceError::NotConnected);
        }
        Ok(DeviceCapabilities {
            model: "DeferredTestBackend".to_owned(),
            serial_number: "SEQUENCE-TEST".to_owned(),
            firmware_version: "test".to_owned(),
            sample_rate_hz: 10.0,
            unit: SampleUnit::VelocityUmPerSec,
            max_record_seconds: 1.0,
        })
    }

    fn stream(
        &mut self,
        request: &StreamRequest,
        on_batch: &mut dyn FnMut(SampleBatch<'_>) -> Result<(), DeviceError>,
        _cancel: &CancelToken,
    ) -> Result<StreamOutcome, DeviceError> {
        self.entered.take().unwrap().send(()).unwrap();
        let (released, changed) = &*self.release;
        let mut guard = released.lock().unwrap_or_else(PoisonError::into_inner);
        while !*guard {
            guard = changed.wait(guard).unwrap_or_else(PoisonError::into_inner);
        }
        drop(guard);

        // Deliberately complete even though reset cancelled this run. The engine's sequence
        // guard, not backend cooperation, must stop this stale result from being published.
        let samples = vec![1.0_f32; request.expected_samples as usize];
        on_batch(SampleBatch {
            samples: &samples,
            start_index: 0,
            arrived_at: Duration::ZERO,
        })?;
        self.returning.take().unwrap().send(()).unwrap();
        Ok(StreamOutcome::Completed)
    }

    fn stop(&self) -> Result<(), DeviceError> {
        Ok(())
    }
}

#[derive(Default)]
struct Recorder {
    lines: Mutex<Vec<String>>,
}

impl NotificationSink for Recorder {
    fn notify(&self, line: &str) {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.to_owned());
    }
}

#[test]
fn late_completion_from_reset_run_cannot_finalize_newer_sequence() {
    let (entered_tx, entered_rx) = mpsc::sync_channel(0);
    let (returning_tx, returning_rx) = mpsc::sync_channel(0);
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let mut backend = DeferredBackend {
        open: false,
        entered: Some(entered_tx),
        returning: Some(returning_tx),
        release: Arc::clone(&release),
    };
    backend
        .open(&DeviceOpenOptions::new(10.0, SampleUnit::VelocityUmPerSec))
        .unwrap();
    let engine = Engine::new(EngineConfig {
        clock,
        logger: Arc::new(NullLogger),
        backend: Box::new(backend),
        last_project: None,
        version: "round2-test".to_owned(),
    });
    let _ = engine.adopt_project(Project::from_json_str(PROJECT).unwrap(), None);
    let recorder = Arc::new(Recorder::default());
    engine.register_session(Arc::clone(&recorder) as Arc<dyn NotificationSink>);

    engine.start_recording().unwrap();
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first reader did not take the backend");

    engine.reset();
    assert_eq!(engine.state(), State::Idle);

    // The original reader still owns the only backend. A newer run therefore finishes as a
    // link loss, establishing sequence N+1 before sequence N is allowed to return.
    engine.start_recording().unwrap();
    assert!(!engine.wait_for_run());
    assert_eq!(engine.state(), State::Aborted);
    assert_eq!(engine.pop_error().error, ScpiError::HardwareError);

    {
        let (released, changed) = &*release;
        *released.lock().unwrap_or_else(PoisonError::into_inner) = true;
        changed.notify_all();
    }
    returning_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("stale reader did not return");
    // `finish_run` follows immediately after backend restoration.
    std::thread::sleep(Duration::from_millis(50));

    assert_eq!(engine.state(), State::Aborted);
    assert_eq!(engine.capture(), Err(ScpiError::DataCorruptOrStale));
    assert_eq!(
        recorder
            .lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1,
        "the stale sequence emitted a second terminal notification"
    );
    assert_eq!(engine.pop_error().error, ScpiError::NoError);
}
