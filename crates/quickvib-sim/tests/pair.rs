//! The documented pair: `quickvib --backend tcp` plus the real `m300-sim` executable.
//!
//! Everything else in the suite drives the simulator's library entry points; this file spawns
//! the built binary and points it at an instrument running in-process on ephemeral ports, so
//! what is under test is the same two processes a UTS integrator would start by hand — minus
//! the hardware, the SDK and the Windows box.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use quickvib::cli::Options;
use quickvib::{AppBuilder, AppHandle};
use quickvib_core::{BackendKind, Level, NullLogger, SystemClock};
use quickvib_testkit::{parse_samples, wait_until, ScpiClient};

/// How long a test will wait for something the other process has to do.
const PATIENCE: Duration = Duration::from_secs(15);

/// A 50 Hz tone at 1 kS/s lands exactly on the peak, so the measurements are analytic:
/// peak `2.0`, RMS `2/sqrt(2)`, peak-to-peak `4.0`.
const RATE_HZ: &str = "1000";
const AMPLITUDE: f64 = 2.0;
const FREQUENCY_HZ: &str = "50";

/// 0.2 s at 1 kHz: 200 samples, which the simulator supplies in about a fifth of a second.
const PROJECT: &str = r#"{
  "schemaVersion": 1,
  "name": "SimPair",
  "device": { "backend": "tcp", "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
  "recording": { "durationSeconds": 0.2 },
  "measurement": { "removeDc": false, "responseDecimals": 4 }
}"#;

/// The simulator built alongside this test.
fn m300_sim() -> Command {
    Command::new(env!("CARGO_BIN_EXE_m300-sim"))
}

/// Kills the child when the test ends, however it ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An instrument listening on ephemeral ports with `--backend tcp` selected.
///
/// Real time, not a test clock: the whole point is that samples arrive from another process at
/// the rate that process chooses.
fn instrument(project_json: &str) -> (AppHandle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("SimPair.proj");
    std::fs::write(&path, project_json).unwrap();

    let app = AppBuilder::new(Options {
        project: Some(path),
        scpi_port: Some(0),
        device_port: Some(0),
        headless: true,
        backend: Some(BackendKind::Tcp),
        no_auto_load: true,
        log_level: Level::Info,
    })
    .bind_host("127.0.0.1")
    .clock(Arc::new(SystemClock::new()))
    .logger(Arc::new(NullLogger))
    .last_project(None)
    .build()
    .expect("the instrument must start");

    (app.spawn(), dir)
}

#[test]
fn the_simulator_binary_prints_help_and_version_and_exits_zero() {
    for flag in ["--help", "--version"] {
        let output = m300_sim().arg(flag).output().unwrap();
        assert!(output.status.success(), "{flag} exited {:?}", output.status);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with("m300-sim "), "{flag} printed: {text}");
    }
}

#[test]
fn a_bad_argument_exits_two_with_usage_on_stderr() {
    let output = m300_sim().arg("--frobnicate").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("unknown option"), "{text}");
    assert!(text.contains("USAGE"), "{text}");
}

#[test]
fn a_device_port_nobody_is_listening_on_exits_three() {
    let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = squatter.local_addr().unwrap().port();
    drop(squatter);

    let output = m300_sim()
        .args(["--host", "127.0.0.1"])
        .args(["--port", &port.to_string()])
        .args(["--duration", "0.01"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("could not connect"));
}

#[test]
fn the_documented_pair_records_and_measures() {
    let (handle, _dir) = instrument(PROJECT);
    let device_port = handle.device_addr().port();

    let sim = Running(
        m300_sim()
            .args(["--host", "127.0.0.1"])
            .args(["--port", &device_port.to_string()])
            .args(["--rate", RATE_HZ])
            .args(["--amplitude", &AMPLITUDE.to_string()])
            .args(["--frequency", FREQUENCY_HZ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let mut uts = ScpiClient::connect(handle.scpi_addr()).unwrap();
    assert_eq!(uts.query("*IDN?").unwrap(), "QuickVib,M300-SCPI,0,1.0.0");
    assert!(
        wait_until(PATIENCE, || uts
            .query("SYST:DEV:CONN?")
            .is_ok_and(|answer| answer == "1")),
        "the simulator must dial in"
    );

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "COMPLETE");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "200");

    let all = uts.query("CALC:MEAS:ALL?").unwrap();
    let fields: Vec<f64> = all
        .split(',')
        .map(|field| field.trim().parse().unwrap())
        .collect();
    assert_eq!(fields.len(), 3, "CALC:MEAS:ALL? returned {all}");

    let (peak, rms, peak_to_peak) = (fields[0], fields[1], fields[2]);
    assert!((peak - AMPLITUDE).abs() < 0.05, "peak {peak}");
    assert!((rms - AMPLITUDE / 2.0_f64.sqrt()).abs() < 0.1, "rms {rms}");
    assert!(
        (peak_to_peak - 2.0 * AMPLITUDE).abs() < 0.1,
        "peak-to-peak {peak_to_peak}"
    );

    let samples = parse_samples(&uts.query("FETC?").unwrap()).unwrap();
    assert_eq!(samples.len(), 200);
    assert!(
        samples
            .iter()
            .all(|s| f64::from(*s).abs() <= AMPLITUDE + 1e-3),
        "every sample must lie on the transmitted sine"
    );
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");

    uts.close();
    drop(sim);
    handle.shutdown();
}

#[test]
fn a_bounded_simulator_run_stops_on_its_own_and_reports_what_it_sent() {
    let (handle, _dir) = instrument(PROJECT);
    let device_port = handle.device_addr().port();

    let output = m300_sim()
        .args(["--host", "127.0.0.1"])
        .args(["--port", &device_port.to_string()])
        .args(["--rate", RATE_HZ])
        .args(["--duration", "0.1"])
        .output()
        .unwrap();

    assert!(output.status.success(), "exited {:?}", output.status);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("sent 100 samples"), "{text}");
    assert!(text.contains("durationElapsed"), "{text}");

    // The instrument saw the link come and go, and stayed up for the next one.
    assert!(wait_until(PATIENCE, || handle.link_stats().connections() == 1));
    assert!(wait_until(PATIENCE, || handle.link_stats().samples() == 100));
    handle.shutdown();
}
