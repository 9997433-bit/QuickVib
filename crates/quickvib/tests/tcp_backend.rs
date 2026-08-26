//! Recording from the inbound device link (`docs/PLAN.md` 7.3, 7.4, 15).
//!
//! `--backend tcp` is the path a real M300 drives: QuickVib listens on `--device-port`, the
//! instrument dials in and pushes little-endian `f32`, and the engine records what arrives.
//! These tests drive that whole route on loopback — accept, frame, buffer, record, measure,
//! export — with no native SDK and no fake DLL anywhere in it, which is what makes it
//! runnable on Linux CI.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{csv_data_rows, Harness, PATIENCE};
use quickvib_core::BackendKind;
use quickvib_testkit::{parse_samples, wait_until, FakeDevice};

/// 0.05 s of velocity at 1 kHz, recorded from whatever dials in.
const TCP_PROJECT: &str = r#"{
  "schemaVersion": 1,
  "name": "InboundDevice",
  "description": "Harness project: 0.05 s velocity capture at 1 kHz over the inbound link.",
  "device": { "backend": "tcp", "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
  "recording": { "durationSeconds": 0.05 },
  "measurement": { "removeDc": false, "responseDecimals": 4 },
  "export": { "format": "CSV", "includeHeader": true },
  "identity": {
    "manufacturer": "QuickVib",
    "model": "M300-SCPI",
    "serialNumber": "SN-0001",
    "firmwareVersion": "1.0.0"
  }
}"#;

/// A 50 Hz tone at 1 kS/s samples the peak exactly, so peak, RMS and peak-to-peak are all
/// analytically known: `2.0`, `2/sqrt(2)` and `4.0`.
const RATE_HZ: f64 = 1000.0;
const AMPLITUDE: f64 = 2.0;
const FREQUENCY_HZ: f64 = 50.0;

fn harness() -> Harness {
    Harness::builder().project_json(TCP_PROJECT).start()
}

/// Wait for the engine to see the link, which is what `INIT` checks.
fn wait_for_device(client: &mut quickvib_testkit::ScpiClient) {
    assert!(
        wait_until(PATIENCE, || client
            .query("SYST:DEV:CONN?")
            .is_ok_and(|answer| answer == "1")),
        "the inbound device must be visible to the engine"
    );
}

#[test]
fn the_tcp_backend_reports_no_device_until_one_dials_in() {
    let harness = harness();
    let mut uts = harness.client();

    assert_eq!(uts.query("SYST:DEV:CONN?").unwrap(), "0");
    // Recording without an instrument is -241, not a hang and not silent mock data.
    uts.command("INIT").unwrap();
    assert_eq!(uts.error_code().unwrap(), -241);

    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);
    device.disconnect();
    assert!(wait_until(PATIENCE, || uts
        .query("SYST:DEV:CONN?")
        .is_ok_and(|answer| answer == "0")));
}

#[test]
fn an_inbound_device_drives_a_full_record_measure_cycle() {
    let harness = harness();
    let mut uts = harness.client();
    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "COMPLETE");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "50");

    let all = uts.query("CALC:MEAS:ALL?").unwrap();
    let fields: Vec<f64> = all
        .split(',')
        .map(|field| field.trim().parse().unwrap())
        .collect();
    assert_eq!(fields.len(), 3, "CALC:MEAS:ALL? returned {all}");

    let (peak, rms, peak_to_peak) = (fields[0], fields[1], fields[2]);
    assert!((peak - AMPLITUDE).abs() < 0.05, "peak {peak}");
    assert!((rms - AMPLITUDE / 2.0_f64.sqrt()).abs() < 0.15, "rms {rms}");
    assert!(
        (peak_to_peak - 2.0 * AMPLITUDE).abs() < 0.1,
        "peak-to-peak {peak_to_peak}"
    );
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");

    device.disconnect();
}

#[test]
fn the_recorded_samples_are_the_ones_that_came_off_the_wire() {
    let harness = harness();
    let mut uts = harness.client();
    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");

    let samples = parse_samples(&uts.query("FETC?").unwrap()).unwrap();
    assert_eq!(samples.len(), 50);
    // Every value has to be on the transmitted sine, whatever chunk boundary it arrived on.
    for sample in &samples {
        assert!(
            f64::from(*sample).abs() <= AMPLITUDE + 1e-3,
            "sample {sample} is off the transmitted waveform"
        );
    }
    assert!(
        samples.iter().any(|s| f64::from(*s).abs() > 0.5),
        "the capture must not be all zeroes"
    );

    device.disconnect();
}

#[test]
fn a_capture_from_the_wire_exports_like_any_other() {
    let harness = harness();
    let mut uts = harness.client();
    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");

    let export = harness.path("inbound.csv");
    uts.command(&format!("MMEM:STOR:TRAC \"{}\"", export.display()))
        .unwrap();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
    assert_eq!(csv_data_rows(&export), 50);

    device.disconnect();
}

#[test]
fn a_device_that_hangs_up_mid_run_aborts_with_240() {
    let harness = harness();
    let mut uts = harness.client();
    // A capture far longer than the device will supply, so the disconnect lands mid-run.
    uts.command("CONF:REC:DUR 30").unwrap();

    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);
    uts.command("INIT").unwrap();
    assert!(wait_until(PATIENCE, || uts
        .query("REC:STAT?")
        .is_ok_and(|state| state == "RECORDING")));

    device.disconnect();

    assert_eq!(uts.query("REC:WAIT?").unwrap(), "0");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "ABORTED");
    assert_eq!(uts.error_code().unwrap(), -240);
    // An aborted run publishes no data at all (D9), so the fetch is -230 rather than an
    // empty capture.
    uts.command("FETC?").unwrap();
    assert_eq!(uts.error_code().unwrap(), -230);
}

#[test]
fn a_second_run_records_fresh_samples_rather_than_replaying_the_first() {
    let harness = harness();
    let mut uts = harness.client();
    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    let first = parse_samples(&uts.query("FETC?").unwrap()).unwrap();

    uts.command("CONF:REC:DUR 0.02").unwrap();
    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "20");
    let second = parse_samples(&uts.query("FETC?").unwrap()).unwrap();

    assert_eq!(first.len(), 50);
    assert_eq!(second.len(), 20);
    assert_ne!(
        first[..20],
        second[..],
        "the second run must capture what arrived after its own INIT"
    );

    device.disconnect();
}

#[test]
fn a_reconnected_device_can_record_again() {
    let harness = harness();
    let mut uts = harness.client();

    let first = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);
    first.disconnect();
    assert!(wait_until(PATIENCE, || uts
        .query("SYST:DEV:CONN?")
        .is_ok_and(|answer| answer == "0")));

    let second = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);
    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "50");

    second.disconnect();
}

#[test]
fn the_command_line_can_select_the_inbound_backend_over_the_project() {
    // The project says `mock`; `--backend tcp` wins, so no device means no data.
    let harness = Harness::builder().backend_kind(BackendKind::Tcp).start();
    let mut uts = harness.client();
    assert_eq!(uts.query("SYST:DEV:CONN?").unwrap(), "0");

    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);
    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "50");

    device.disconnect();
}

#[test]
fn a_second_inbound_device_is_still_refused_while_one_is_recording() {
    let harness = harness();
    let mut uts = harness.client();
    let device = harness.streaming_device(RATE_HZ, AMPLITUDE, FREQUENCY_HZ);
    wait_for_device(&mut uts);

    let mut intruder = FakeDevice::connect(harness.handle.device_addr()).unwrap();
    assert!(intruder.was_refused(Duration::from_secs(2)).unwrap());
    assert!(wait_until(PATIENCE, || harness
        .handle
        .link_stats()
        .refusals()
        == 1));

    // The live link is undisturbed, so a run still completes.
    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");

    device.disconnect();
}
