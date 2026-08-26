//! Wire-protocol robustness (`docs/PLAN.md` 7.2, 17.2).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::Harness;

#[test]
fn both_line_terminators_are_accepted() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.send_raw(b"*IDN?\n").unwrap();
    let with_lf = uts.read_response().unwrap();

    uts.send_raw(b"*IDN?\r\n").unwrap();
    let with_crlf = uts.read_response().unwrap();

    assert_eq!(with_lf, with_crlf);
    assert_eq!(with_lf.split(',').count(), 4);
}

#[test]
fn a_compound_message_answers_on_one_line() {
    let harness = Harness::start();
    let mut uts = harness.client();

    let response = uts.query("*CLS;*IDN?").unwrap();
    assert_eq!(response.split(',').count(), 4);

    assert_eq!(uts.query("FORM?;REC:STAT?").unwrap(), "CSV;IDLE");
}

#[test]
fn a_compound_message_keeps_the_scpi_header_path() {
    let harness = Harness::start();
    let mut uts = harness.client();

    assert_eq!(uts.query("CONF:REC:DUR 0.25;DUR?").unwrap(), "0.250");
    assert_eq!(uts.query("CONF:REC:DUR 0.5;:FORM?").unwrap(), "CSV");
    assert_eq!(uts.query("CONF:REC:DUR?").unwrap(), "0.500");
}

#[test]
fn headers_are_case_insensitive_and_accept_both_forms() {
    let harness = Harness::start();
    let mut uts = harness.client();

    let canonical = uts.query("*IDN?").unwrap();
    assert_eq!(uts.query("*idn?").unwrap(), canonical);
    assert_eq!(
        uts.query("configure:record:duration?").unwrap(),
        uts.query("CONF:REC:DUR?").unwrap()
    );
}

#[test]
fn an_empty_line_is_ignored_rather_than_treated_as_an_error() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.send_raw(b"\n").unwrap();
    uts.send_raw(b"   \r\n").unwrap();
    assert_eq!(uts.query("*IDN?").unwrap().split(',').count(), 4);
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
}

#[test]
fn a_command_produces_no_response_line_at_all() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("*CLS").unwrap();
    uts.command("ABOR").unwrap();
    assert!(uts.expect_silence(Duration::from_millis(200)).unwrap());

    // …and the session is still perfectly usable afterwards.
    assert_eq!(uts.query("*IDN?").unwrap().split(',').count(), 4);
}

#[test]
fn several_messages_arriving_in_one_packet_are_answered_in_order() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.send_raw(b"FORM?\nREC:STAT?\nSYST:VERS?\n").unwrap();
    assert_eq!(uts.read_response().unwrap(), "CSV");
    assert_eq!(uts.read_response().unwrap(), "IDLE");
    assert_eq!(uts.read_response().unwrap(), "1999.0");
}

#[test]
fn a_quoted_path_may_contain_a_semicolon() {
    let harness = Harness::start();
    let mut uts = harness.client();

    let odd = harness.path("a;b.proj");
    uts.command(&format!("MMEM:LOAD:STAT \"{}\"", odd.display()))
        .unwrap();
    // The whole quoted string reached the loader, so this is a missing file rather than a
    // parse failure.
    assert_eq!(uts.error_code().unwrap(), -256);
}

#[test]
fn a_failure_inside_a_compound_message_stops_the_rest_of_the_line() {
    let harness = Harness::start();
    let mut uts = harness.client();

    uts.command("*CLS;NOPE?;FORM TXT").unwrap();
    assert_eq!(uts.error_code().unwrap(), -113);
    // `FORM TXT` was never executed.
    assert_eq!(uts.query("FORM?").unwrap(), "CSV");
}
