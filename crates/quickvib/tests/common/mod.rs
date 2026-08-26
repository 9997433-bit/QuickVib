//! Shared harness for the integration suite (`docs/PLAN.md` 17.2).
//!
//! Every test binds `127.0.0.1:0` and works inside a `tempfile::TempDir`, so the whole suite
//! is parallel-safe under `cargo test`'s default thread-per-test model — no serialization
//! attribute and no fixed ports anywhere.

// The module is `include`d by every test binary, so each one uses a different subset of it,
// and `pub` here means "visible to this test binary" rather than "part of an API".
#![allow(dead_code, unreachable_pub, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use quickvib::cli::Options;
use quickvib::{AppBuilder, AppHandle};
use quickvib_core::{BackendKind, Clock, Level, NullLogger, SystemClock, TestClock};
use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend, MockFault};
use quickvib_project::LastProjectStore;
use quickvib_testkit::{FakeDevice, ScpiClient, StreamingDevice};
use tempfile::TempDir;

/// How long a test will wait for something a concurrent thread has to do.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// A project small enough that a capture is over before the assertion after it.
pub const PROJECT: &str = r#"{
  "schemaVersion": 1,
  "name": "Integration",
  "description": "Harness project: 0.05 s velocity capture at 1 kHz.",
  "device": { "backend": "mock", "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
  "recording": { "durationSeconds": 0.05 },
  "measurement": { "removeDc": false, "responseDecimals": 4 },
  "export": { "format": "CSV", "includeHeader": true },
  "identity": {
    "manufacturer": "QuickVib",
    "model": "M300-SCPI",
    "serialNumber": "SN-0001",
    "firmwareVersion": "1.0.0"
  },
  "mock": {
    "signal": {
      "components": [ { "frequencyHz": 50.0, "amplitude": 2.0, "phaseDeg": 0.0 } ],
      "noiseStdDev": 0.0,
      "seed": 12345
    }
  }
}"#;

/// A running instrument plus the temporary directory its files live in.
pub struct Harness {
    /// The running server. Dropping it stops both accept loops.
    pub handle: AppHandle,
    /// Scratch directory for projects and exports.
    pub dir: TempDir,
    /// Where [`PROJECT`] was written, whether or not it was loaded at startup.
    pub project_path: PathBuf,
}

/// Assembles a [`Harness`].
pub struct HarnessBuilder {
    project_loaded: bool,
    project_json: String,
    clock: Arc<dyn Clock>,
    backend: Option<Box<dyn DeviceBackend + Send>>,
    backend_kind: Option<BackendKind>,
    max_sessions: Option<usize>,
    fault_injection: bool,
    auto_load: bool,
}

impl Default for HarnessBuilder {
    fn default() -> Self {
        Self {
            project_loaded: true,
            project_json: PROJECT.to_owned(),
            // A virtual clock is what lets a paced capture finish instantly (D20).
            clock: Arc::new(TestClock::at_epoch()),
            backend: None,
            backend_kind: None,
            max_sessions: None,
            fault_injection: false,
            auto_load: false,
        }
    }
}

impl HarnessBuilder {
    /// Start with nothing loaded, so project-dependent commands report `-221`.
    pub fn without_project(mut self) -> Self {
        self.project_loaded = false;
        self
    }

    /// Use a different project document.
    pub fn project_json(mut self, json: impl Into<String>) -> Self {
        self.project_json = json.into();
        self
    }

    /// Drive the instrument from real time, for tests that need to observe a capture in
    /// progress rather than after the fact.
    pub fn real_time(mut self) -> Self {
        self.clock = Arc::new(SystemClock::new());
        self
    }

    /// Substitute a backend, typically one carrying an injected fault.
    pub fn backend(mut self, backend: Box<dyn DeviceBackend + Send>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Override the backend the way `--backend <kind>` does on the command line.
    pub fn backend_kind(mut self, kind: BackendKind) -> Self {
        self.backend_kind = Some(kind);
        self
    }

    /// Lower the concurrent session cap.
    pub fn max_sessions(mut self, max: usize) -> Self {
        self.max_sessions = Some(max);
        self
    }

    /// Enable the test-only fault-injection command.
    pub fn fault_injection(mut self) -> Self {
        self.fault_injection = true;
        self
    }

    /// Record the project as the last one used and let startup auto-load it.
    pub fn auto_load(mut self) -> Self {
        self.auto_load = true;
        self.project_loaded = false;
        self
    }

    /// Write the project, bind both ports and start the server.
    pub fn start(self) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("Integration.proj");
        std::fs::write(&project_path, &self.project_json).unwrap();

        let last_project = LastProjectStore::new(dir.path().join("state"));
        if self.auto_load {
            last_project.record(&project_path).unwrap();
        }

        let options = Options {
            project: self.project_loaded.then(|| project_path.clone()),
            scpi_port: Some(0),
            device_port: Some(0),
            bind_host: None,
            headless: true,
            backend: self.backend_kind,
            no_auto_load: !self.auto_load,
            log_level: Level::Info,
        };

        let mut builder = AppBuilder::new(options)
            .bind_host("127.0.0.1")
            .clock(Arc::clone(&self.clock))
            .logger(Arc::new(NullLogger))
            .last_project(Some(last_project))
            .fault_injection(self.fault_injection);
        if let Some(backend) = self.backend {
            builder = builder.backend(backend);
        }
        if let Some(max) = self.max_sessions {
            builder = builder.max_sessions(max);
        }

        let app = builder.build().expect("the instrument must start");
        Harness {
            handle: app.spawn(),
            dir,
            project_path,
        }
    }
}

impl Harness {
    /// A harness with the default project already loaded.
    pub fn start() -> Self {
        HarnessBuilder::default().start()
    }

    /// A builder, for tests that need something other than the defaults.
    pub fn builder() -> HarnessBuilder {
        HarnessBuilder::default()
    }

    /// Open a SCPI session and wait for the server to finish setting it up.
    ///
    /// The handshake is not decoration: a session is registered for notifications by its own
    /// thread, so a test that connects and immediately starts a recording could otherwise
    /// race the registration and miss `#REC:DONE`.
    pub fn client(&self) -> ScpiClient {
        let mut client = ScpiClient::connect(self.handle.scpi_addr())
            .expect("the SCPI port must accept sessions");
        client
            .query("*IDN?")
            .expect("a new session must answer the identity query");
        client
    }

    /// Dial into the device port as an M300 would.
    pub fn device(&self) -> FakeDevice {
        FakeDevice::connect(self.handle.device_addr()).expect("the device port must be open")
    }

    /// Dial in and keep streaming, as a powered-on M300 — or `m300-sim` — does.
    pub fn streaming_device(
        &self,
        sample_rate_hz: f64,
        amplitude: f64,
        frequency_hz: f64,
    ) -> StreamingDevice {
        StreamingDevice::connect(
            self.handle.device_addr(),
            sample_rate_hz,
            amplitude,
            frequency_hz,
        )
        .expect("the device port must be open")
    }

    /// A path inside the scratch directory.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The scratch directory itself.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

/// A mock backend with an injected fault, already opened at `rate_hz`.
pub fn faulty_backend(
    clock: Arc<dyn Clock>,
    fault: MockFault,
    rate_hz: f64,
) -> Box<dyn DeviceBackend + Send> {
    let mut backend = MockBackend::new(clock).with_fault(fault);
    backend
        .open(&DeviceOpenOptions::new(
            rate_hz,
            quickvib_core::SampleUnit::VelocityUmPerSec,
        ))
        .expect("the mock must open");
    Box::new(backend)
}

/// Count the rows a CSV export produced, excluding the preamble and header.
pub fn csv_data_rows(path: &Path) -> usize {
    let text = std::fs::read_to_string(path).expect("the export must be readable");
    text.lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty()
                && !line.starts_with('#')
                && line
                    .split(',')
                    .next()
                    .is_some_and(|f| f.parse::<f64>().is_ok())
        })
        .count()
}
