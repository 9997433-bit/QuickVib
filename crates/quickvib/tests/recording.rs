//! Recording control: notifications, polling, abort and the watchdog
//! (`docs/PLAN.md` 7.6, 8.4, 8.8).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{faulty_backend, Harness, PATIENCE};
use quickvib_core::{Clock, SystemClock, TestClock};
use quickvib_device::MockFault;
use quickvib_scpi::{NOTIFY_RECORD_ABORTED, NOTIFY_RECORD_DONE};

#[test]
fn rec_done_arrives_on_a_session_that_is_only_reading() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(
        uts.wait_notification(PATIENCE).unwrap().as_deref(),
        Some(NOTIFY_RECORD_DONE)
    );
}

#[test]
fn the_notification_is_sent_before_rec_wait_unblocks() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    // A UTS that uses both must never see them out of order, so by the time the blocking
    // query answers, the notification is already on the wire ahead of it.
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.take_notification().as_deref(), Some(NOTIFY_RECORD_DONE));
}

#[test]
fn every_session_sees_the_notification() {
    let harness = Harness::start();
    let mut first = harness.client();
    let mut second = harness.client();

    first.command("INIT").unwrap();

    assert_eq!(
        first.wait_notification(PATIENCE).unwrap().as_deref(),
        Some(NOTIFY_RECORD_DONE)
    );
    assert_eq!(
        second.wait_notification(PATIENCE).unwrap().as_deref(),
        Some(NOTIFY_RECORD_DONE)
    );
}

#[test]
fn a_polling_client_observes_the_run_progress_through_to_complete() {
    // Real time and 30 000 samples: enough batches that `RECORDING` is genuinely observable
    // rather than a state the run passes through between two polls.
    let harness = Harness::builder()
        .real_time()
        .project_json(
            r#"{
              "schemaVersion": 1,
              "name": "Polling",
              "device": { "sampleRateHz": 100000.0, "unit": "velocity_um_s" },
              "recording": { "durationSeconds": 0.3 },
              "mock": { "signal": { "components": [
                { "frequencyHz": 120.0, "amplitude": 5.0, "phaseDeg": 0.0 } ], "seed": 3 } }
            }"#,
        )
        .start();
    let mut uts = harness.client();

    assert_eq!(uts.query("REC:STAT?").unwrap(), "IDLE");
    uts.command("INIT").unwrap();

    let mut seen = Vec::new();
    for _ in 0..2_000 {
        let state = uts.query("REC:STAT?").unwrap();
        if seen.last() != Some(&state) {
            seen.push(state.clone());
        }
        if state == "COMPLETE" {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    assert_eq!(
        seen.last().map(String::as_str),
        Some("COMPLETE"),
        "{seen:?}"
    );
    assert!(seen.contains(&"RECORDING".to_owned()), "{seen:?}");
    assert!(
        seen.iter()
            .all(|s| ["ARMED", "RECORDING", "COMPLETE"].contains(&s.as_str())),
        "{seen:?}"
    );
}

#[test]
fn abort_mid_capture_leaves_no_data_behind() {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let harness = Harness::builder()
        .backend(faulty_backend(clock, MockFault::Stall, 1000.0))
        .start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    uts.command("ABOR").unwrap();

    assert_eq!(uts.query("REC:WAIT?").unwrap(), "0");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "ABORTED");

    uts.command("FETC?").unwrap();
    assert_eq!(uts.error_code().unwrap(), -230);
    // An operator abort is a state, not a fault, so nothing else is queued.
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
}

#[test]
fn an_aborted_run_notifies_rather_than_going_silent() {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let harness = Harness::builder()
        .backend(faulty_backend(clock, MockFault::Stall, 1000.0))
        .start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    uts.command("ABOR").unwrap();
    assert_eq!(
        uts.wait_notification(PATIENCE).unwrap().as_deref(),
        Some(NOTIFY_RECORD_ABORTED)
    );
}

#[test]
fn a_stalled_device_trips_the_watchdog_with_365() {
    // The watchdog is `duration * multiplier + 1 s`, so this takes just over a second of real
    // time no matter which clock the instrument runs on.
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let harness = Harness::builder()
        .real_time()
        .backend(faulty_backend(clock, MockFault::Stall, 1000.0))
        .project_json(
            r#"{
              "schemaVersion": 1,
              "name": "Watchdog",
              "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
              "recording": { "durationSeconds": 0.01, "timeoutMultiplier": 2.0 }
            }"#,
        )
        .start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "0");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "ABORTED");
    assert_eq!(uts.error_code().unwrap(), -365);
}

#[test]
fn a_link_that_drops_mid_capture_is_240() {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let harness = Harness::builder()
        .backend(faulty_backend(clock, MockFault::LinkLostAfter(10), 1000.0))
        .start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "0");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "ABORTED");
    assert_eq!(uts.error_code().unwrap(), -240);
}

#[test]
fn a_short_stream_is_240_rather_than_a_partial_capture() {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let harness = Harness::builder()
        .backend(faulty_backend(clock, MockFault::ShortStream(10), 1000.0))
        .start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "0");
    assert_eq!(uts.error_code().unwrap(), -240);
    uts.command("FETC?").unwrap();
    assert_eq!(uts.error_code().unwrap(), -230);
}

#[test]
fn rec_star_is_an_alias_for_init() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("REC:STAR").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "50");
}

#[test]
fn a_second_run_replaces_the_previous_capture() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "50");

    uts.command("CONF:REC:DUR 0.02").unwrap();
    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "20");
}

#[test]
fn opc_resolves_once_the_run_is_over() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("*OPC").unwrap();
    uts.command("INIT").unwrap();
    assert_eq!(uts.query("*OPC?").unwrap(), "1");
    assert_eq!(uts.query("REC:STAT?").unwrap(), "COMPLETE");
}
