//! Multiple concurrent sessions against one shared instrument (`docs/PLAN.md` 5.4, 7.2, D8).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::{Harness, PATIENCE};
use quickvib_scpi::NOTIFY_RECORD_DONE;
use quickvib_testkit::{wait_until, ScpiClient};

#[test]
fn two_sessions_share_one_instrument_model() {
    let harness = Harness::start();
    let mut first = harness.client();
    let mut second = harness.client();

    // Reading it back on the first session is what orders the two: nothing synchronises
    // separate sessions, so the second one is only guaranteed to see a setting once the
    // session that made the change has observed it.
    first.command("CONF:REC:DUR 0.02").unwrap();
    assert_eq!(first.query("CONF:REC:DUR?").unwrap(), "0.020");
    assert_eq!(second.query("CONF:REC:DUR?").unwrap(), "0.020");

    // Same ordering hazard, and `INIT` answers nothing to order against: until the session
    // that issued it has observed the state leave IDLE, `REC:WAIT?` on the other session is
    // entitled to report that no run was ever started.
    second.command("INIT").unwrap();
    assert_ne!(second.query("REC:STAT?").unwrap(), "IDLE");

    assert_eq!(first.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(first.query("TRAC:POIN?").unwrap(), "20");
    assert_eq!(second.query("TRAC:POIN?").unwrap(), "20");
}

#[test]
fn each_session_resolves_its_own_opc_query() {
    let harness = Harness::start();
    let mut first = harness.client();
    let mut second = harness.client();

    first.command("*OPC").unwrap();
    first.command("INIT").unwrap();
    assert_eq!(first.query("*OPC?").unwrap(), "1");
    assert_eq!(second.query("*OPC?").unwrap(), "1");
}

#[test]
fn a_notification_never_lands_inside_another_sessions_response() {
    let harness = Harness::start();
    let mut runner = harness.client();
    let mut chatterer = harness.client();

    let identity = chatterer.query("*IDN?").unwrap();
    let stop = Arc::new(AtomicBool::new(false));

    let reader_stop = Arc::clone(&stop);
    let reader = std::thread::spawn(move || {
        let mut lines = 0_u32;
        while !reader_stop.load(Ordering::Relaxed) {
            // Every response must arrive whole: an interleaved `#REC:DONE` would show up
            // either as a corrupt identity or as a notification the client cannot skip.
            let response = chatterer
                .query("*IDN?")
                .expect("the session must stay usable");
            assert_eq!(response, identity);
            lines += 1;
        }
        lines
    });

    for _ in 0..6 {
        runner.command("INIT").unwrap();
        assert_eq!(runner.query("REC:WAIT?").unwrap(), "1");
    }

    stop.store(true, Ordering::Relaxed);
    let lines = reader.join().unwrap();
    assert!(lines > 0);
}

#[test]
fn the_ninth_session_is_refused_and_the_first_eight_are_undisturbed() {
    let harness = Harness::builder().max_sessions(8).start();

    let mut sessions: Vec<ScpiClient> = (0..8).map(|_| harness.client()).collect();
    for session in &mut sessions {
        assert_eq!(session.query("*IDN?").unwrap().split(',').count(), 4);
    }
    assert!(wait_until(PATIENCE, || harness
        .handle
        .engine()
        .session_count()
        == 8));

    // The connection is accepted by the OS and then closed by the server, so the ninth
    // client sees an immediate clean end of stream rather than a hang.
    let mut refused =
        ScpiClient::connect_with_timeout(harness.handle.scpi_addr(), Duration::from_secs(5))
            .unwrap();
    refused.send("*IDN?").unwrap();
    assert!(refused.read_response().is_err());

    for session in &mut sessions {
        assert_eq!(session.query("*IDN?").unwrap().split(',').count(), 4);
    }
}

#[test]
fn a_closed_session_frees_its_slot() {
    let harness = Harness::builder().max_sessions(2).start();

    let first = harness.client();
    let mut second = harness.client();
    assert!(wait_until(PATIENCE, || harness
        .handle
        .engine()
        .session_count()
        == 2));

    first.close();
    assert!(wait_until(PATIENCE, || harness
        .handle
        .engine()
        .session_count()
        == 1));

    let mut third = harness.client();
    assert_eq!(third.query("*IDN?").unwrap().split(',').count(), 4);
    assert_eq!(second.query("*IDN?").unwrap().split(',').count(), 4);
}

#[test]
fn a_client_that_vanishes_mid_query_disturbs_nobody() {
    let harness = Harness::start();
    let mut survivor = harness.client();

    {
        let mut quitter = harness.client();
        quitter.send("FETC?").unwrap();
        // Drop the socket without reading the response.
    }

    assert!(wait_until(PATIENCE, || harness
        .handle
        .engine()
        .session_count()
        == 1));
    assert_eq!(survivor.query("*IDN?").unwrap().split(',').count(), 4);

    survivor.command("INIT").unwrap();
    assert_eq!(survivor.query("REC:WAIT?").unwrap(), "1");
}

#[test]
fn a_session_that_panics_is_closed_and_the_process_stays_healthy() {
    let harness = Harness::builder().fault_injection().start();
    let mut survivor = harness.client();

    let mut doomed = harness.client();
    doomed.send("SYST:TEST:PANIC").unwrap();
    assert!(doomed.read_response().is_err());
    assert!(wait_until(PATIENCE, || harness
        .handle
        .engine()
        .session_count()
        == 1));

    // The contained panic is reported through the standard channel, not by dying.
    assert_eq!(survivor.error_code().unwrap(), -100);

    survivor.command("INIT").unwrap();
    assert_eq!(survivor.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(
        survivor.wait_notification(PATIENCE).unwrap().as_deref(),
        Some(NOTIFY_RECORD_DONE)
    );
}
