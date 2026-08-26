//! The error catalogue as the UTS observes it (`docs/PLAN.md` 9, 17.2).
//!
//! A failure never produces a response line: it pushes a SCPI-99 code the UTS collects with
//! `SYST:ERR?`, which is standard instrument behaviour and keeps commands and queries alike.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::Harness;

#[test]
fn a_fresh_instrument_has_an_empty_error_queue() {
    let harness = Harness::start();
    let mut uts = harness.client();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
}

#[test]
fn init_without_a_project_is_221() {
    let harness = Harness::builder().without_project().start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.error_code().unwrap(), -221);
    assert_eq!(uts.query("REC:STAT?").unwrap(), "IDLE");
}

#[test]
fn an_instrument_without_a_project_still_answers_the_mandated_queries() {
    let harness = Harness::builder().without_project().start();
    let mut uts = harness.client();

    assert_eq!(uts.query("*IDN?").unwrap().split(',').count(), 4);
    assert_eq!(uts.query("SYST:VERS?").unwrap(), "1999.0");
    assert_eq!(uts.query("*OPC?").unwrap(), "1");
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
}

#[test]
fn a_data_query_before_any_run_is_230() {
    let harness = Harness::start();
    let mut uts = harness.client();

    for query in [
        "FETC?",
        "TRAC:DATA?",
        "TRAC:POIN?",
        "CALC:MEAS:PEAK?",
        "CALC:MEAS:RMS?",
        "CALC:MEAS:PP?",
        "CALC:MEAS:ALL?",
    ] {
        uts.command(query).unwrap();
        assert_eq!(uts.error_code().unwrap(), -230, "for {query}");
    }
}

#[test]
fn loading_a_project_that_is_not_there_is_256() {
    let harness = Harness::start();
    let mut uts = harness.client();

    let missing = harness.path("definitely-not-here.proj");
    uts.command(&format!("MMEM:LOAD:STAT \"{}\"", missing.display()))
        .unwrap();
    assert_eq!(uts.error_code().unwrap(), -256);
}

#[test]
fn loading_a_malformed_project_is_224() {
    let harness = Harness::start();
    let mut uts = harness.client();

    let broken = harness.path("Broken.proj");
    std::fs::write(&broken, "{ this is not json").unwrap();
    uts.command(&format!("MMEM:LOAD:STAT \"{}\"", broken.display()))
        .unwrap();
    assert_eq!(uts.error_code().unwrap(), -224);
}

#[test]
fn an_unknown_export_format_is_224() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("FORM XML").unwrap();
    assert_eq!(uts.error_code().unwrap(), -224);
    // The rejected value never becomes the active format.
    assert_eq!(uts.query("FORM?").unwrap(), "CSV");
}

#[test]
fn a_duration_outside_the_allowed_range_is_222() {
    let harness = Harness::start();
    let mut uts = harness.client();

    for value in ["0", "-1", "3600.1"] {
        uts.command(&format!("CONF:REC:DUR {value}")).unwrap();
        assert_eq!(uts.error_code().unwrap(), -222, "for {value}");
    }
    assert_eq!(uts.query("CONF:REC:DUR?").unwrap(), "0.050");
}

#[test]
fn an_unknown_header_is_113_with_no_response() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("NOPE?").unwrap();
    assert_eq!(uts.error_code().unwrap(), -113);

    uts.command("MEAS:NOTHING").unwrap();
    assert_eq!(uts.error_code().unwrap(), -113);
}

#[test]
fn a_malformed_message_is_100() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.send_raw(&[0xFF, 0xFE, b'\n']).unwrap();
    assert_eq!(uts.error_code().unwrap(), -100);

    uts.command("CONF:REC:DUR not-a-number").unwrap();
    assert!(uts.error_code().unwrap() < 0);
}

#[test]
fn an_over_long_line_is_100_and_the_session_survives() {
    let harness = Harness::start();
    let mut uts = harness.client();

    let mut monster = vec![b'A'; 70 * 1024];
    monster.push(b'\n');
    uts.send_raw(&monster).unwrap();

    assert_eq!(uts.error_code().unwrap(), -100);
    assert_eq!(uts.query("*IDN?").unwrap().split(',').count(), 4);
}

#[test]
fn a_config_change_during_a_run_is_221() {
    let clock: std::sync::Arc<dyn quickvib_core::Clock> =
        std::sync::Arc::new(quickvib_core::TestClock::at_epoch());
    let harness = Harness::builder()
        .backend(common::faulty_backend(
            clock,
            quickvib_device::MockFault::Stall,
            1000.0,
        ))
        .start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    for command in ["CONF:REC:DUR 1.0", "FORM TXT", "INIT"] {
        uts.command(command).unwrap();
        assert_eq!(uts.error_code().unwrap(), -221, "for {command}");
    }

    uts.command("ABOR").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "0");
}

#[test]
fn the_error_queue_is_first_in_first_out_and_drains_to_no_error() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("CONF:REC:DUR 0").unwrap();
    uts.command("FORM XML").unwrap();
    uts.command("NOPE?").unwrap();

    assert_eq!(uts.error_code().unwrap(), -222);
    assert_eq!(uts.error_code().unwrap(), -224);
    assert_eq!(uts.error_code().unwrap(), -113);
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
}

#[test]
fn cls_empties_the_error_queue() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("CONF:REC:DUR 0").unwrap();
    uts.command("*CLS").unwrap();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
}

#[test]
fn the_error_queue_reports_overflow_rather_than_dropping_the_newest_silently() {
    let harness = Harness::start();
    let mut uts = harness.client();

    for _ in 0..40 {
        uts.command("NOPE?").unwrap();
    }
    // Let the last of the pipelined commands land before draining.
    uts.set_read_timeout(Duration::from_secs(5)).unwrap();

    let mut codes = Vec::new();
    loop {
        let code = uts.error_code().unwrap();
        if code == 0 {
            break;
        }
        codes.push(code);
    }
    assert!(codes.contains(&-350), "{codes:?}");
    assert!(codes.len() <= 32, "queue depth exceeded: {}", codes.len());
}
