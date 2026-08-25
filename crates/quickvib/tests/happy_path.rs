//! The sequence a UTS actually runs, over a real loopback socket (`docs/PLAN.md` 5.2, 17.2).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{csv_data_rows, Harness};
use quickvib_testkit::parse_samples;

#[test]
fn the_documented_uts_sequence_runs_end_to_end() {
    let harness = Harness::start();
    let mut uts = harness.client();

    assert_eq!(
        uts.query("*IDN?").unwrap(),
        "QuickVib,M300-SCPI,SN-0001,1.0.0"
    );

    uts.command(&format!(
        "MMEM:LOAD:STAT \"{}\"",
        harness.project_path.display()
    ))
    .unwrap();
    uts.command("CONF:REC:DUR 0.1").unwrap();
    assert_eq!(uts.query("CONF:REC:DUR?").unwrap(), "0.100");
    assert_eq!(uts.query("SYST:DEV:CONN?").unwrap(), "1");

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "COMPLETE");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "100");

    let measurements: Vec<f64> = uts
        .query("CALC:MEAS:ALL?")
        .unwrap()
        .split(',')
        .map(|field| field.parse().unwrap())
        .collect();
    assert_eq!(measurements.len(), 3);
    // The mock is a 2 um/s sine, so peak, RMS and peak-to-peak are analytically known.
    assert!(
        (measurements[0] - 2.0).abs() < 0.05,
        "peak {measurements:?}"
    );
    assert!(
        (measurements[1] - 2.0 / 2.0_f64.sqrt()).abs() < 0.05,
        "rms {measurements:?}"
    );
    assert!(
        (measurements[2] - 4.0).abs() < 0.1,
        "peak-to-peak {measurements:?}"
    );

    let export = harness.path("run001.csv");
    uts.command(&format!("MMEM:STOR:TRAC \"{}\"", export.display()))
        .unwrap();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
    assert!(export.is_file());
    assert_eq!(csv_data_rows(&export), 100);
}

#[test]
fn the_scalar_measurement_queries_agree_with_the_combined_one() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");

    let all = uts.query("CALC:MEAS:ALL?").unwrap();
    let fields: Vec<&str> = all.split(',').collect();
    assert_eq!(fields[0], uts.query("CALC:MEAS:PEAK?").unwrap());
    assert_eq!(fields[1], uts.query("CALC:MEAS:RMS?").unwrap());
    assert_eq!(fields[2], uts.query("CALC:MEAS:PP?").unwrap());
}

#[test]
fn fetch_and_trace_data_return_the_same_capture() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");

    let fetched = parse_samples(&uts.query("FETC?").unwrap()).unwrap();
    let traced = parse_samples(&uts.query("TRAC:DATA?").unwrap()).unwrap();
    assert_eq!(fetched.len(), 50);
    assert_eq!(fetched, traced);
    // The mock is deterministic, so the first sample of a zero-phase sine is exactly zero.
    assert_eq!(fetched[0], 0.0);
}

#[test]
fn a_large_capture_transfers_completely_and_parses_back() {
    let harness = Harness::builder()
        .project_json(
            r#"{
              "schemaVersion": 1,
              "name": "AtScale",
              "device": { "sampleRateHz": 100000.0, "unit": "velocity_um_s" },
              "recording": { "durationSeconds": 1.0 },
              "mock": { "signal": { "components": [
                { "frequencyHz": 120.0, "amplitude": 250.0, "phaseDeg": 0.0 } ], "seed": 1 } }
            }"#,
        )
        .start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "100000");

    let samples = parse_samples(&uts.query("FETC?").unwrap()).unwrap();
    assert_eq!(samples.len(), 100_000);
    assert!(samples.iter().all(|s| s.abs() <= 251.0));
}

#[test]
fn the_export_format_can_be_switched_to_txt() {
    let harness = Harness::start();
    let mut uts = harness.client();

    assert_eq!(uts.query("FORM?").unwrap(), "CSV");
    uts.command("FORM TXT").unwrap();
    assert_eq!(uts.query("FORM?").unwrap(), "TXT");

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");

    let export = harness.path("run001.txt");
    uts.command(&format!("MMEM:STOR:TRAC \"{}\"", export.display()))
        .unwrap();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");

    let text = std::fs::read_to_string(&export).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 50);
    assert!(lines.iter().all(|line| line.parse::<f64>().is_ok()));
}

#[test]
fn a_project_saved_at_runtime_keeps_the_overrides() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("CONF:REC:DUR 0.25").unwrap();
    uts.command("FORM TXT").unwrap();

    let saved = harness.path("Saved.proj");
    uts.command(&format!("MMEM:STOR:STAT \"{}\"", saved.display()))
        .unwrap();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");

    uts.command("*RST").unwrap();
    uts.command(&format!("MMEM:LOAD:STAT \"{}\"", saved.display()))
        .unwrap();
    assert_eq!(uts.query("CONF:REC:DUR?").unwrap(), "0.250");
    assert_eq!(uts.query("FORM?").unwrap(), "TXT");
}

#[test]
fn auto_load_reopens_the_last_project_at_startup_and_on_demand() {
    let harness = Harness::builder().auto_load().start();
    let mut uts = harness.client();

    assert_eq!(
        uts.query("*IDN?").unwrap(),
        "QuickVib,M300-SCPI,SN-0001,1.0.0"
    );
    assert!(uts
        .query("MMEM:LOAD:AUTO?")
        .unwrap()
        .contains("Integration"));

    uts.command("*RST").unwrap();
    uts.command("MMEM:LOAD:AUTO").unwrap();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
    assert_eq!(uts.query("CONF:REC:DUR?").unwrap(), "0.050");
}

#[test]
fn reset_returns_the_instrument_to_a_known_state() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    uts.command("CONF:REC:DUR 2.0").unwrap();
    uts.command("FORM TXT").unwrap();

    uts.command("*RST").unwrap();

    assert_eq!(uts.query("REC:STAT?").unwrap(), "IDLE");
    assert_eq!(uts.query("CONF:REC:DUR?").unwrap(), "0.050");
    assert_eq!(uts.query("FORM?").unwrap(), "CSV");
    // *RST discards the capture, so a data query is stale again.
    uts.command("FETC?").unwrap();
    assert_eq!(uts.error_code().unwrap(), -230);
}
