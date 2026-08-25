//! The inbound device link (`docs/PLAN.md` 7.3).
//!
//! On the mock path there is no real M300, so these tests dial into the device port with a
//! stand-in that pushes little-endian `f32` bytes. What is under test is the accept, refusal
//! and framing behaviour the M300 backend will depend on, exercised on Linux where CI can
//! actually run it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Harness, PATIENCE};
use quickvib_testkit::{wait_until, FakeDevice};

#[test]
fn the_device_port_is_open_and_accepts_a_stream() {
    let harness = Harness::start();
    let stats = harness.handle.link_stats();
    let state = harness.handle.link_state();

    let mut device = harness.device();
    assert!(state.wait_connected(PATIENCE));
    // The listener publishes the state; the handler thread counts the connection a moment
    // later.
    assert!(wait_until(PATIENCE, || stats.connections() == 1));

    device.push(&[1.0, -2.0, 3.5, 4.25]).unwrap();
    assert!(wait_until(PATIENCE, || stats.samples() == 4));

    device.disconnect();
    assert!(!state.wait_disconnected(PATIENCE));
    assert_eq!(stats.refusals(), 0);
}

#[test]
fn samples_split_across_packets_are_reassembled() {
    let harness = Harness::start();
    let stats = harness.handle.link_stats();

    let mut device = harness.device();
    let bytes = 42.5_f32.to_le_bytes();
    device.push_bytes(&bytes[..1]).unwrap();
    device.push_bytes(&bytes[1..3]).unwrap();
    // Three of the four bytes can never make a sample, however the packets landed.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(stats.samples(), 0);

    device.push_bytes(&bytes[3..]).unwrap();

    assert!(wait_until(PATIENCE, || stats.samples() == 1));
    device.disconnect();
}

#[test]
fn a_second_device_is_refused_while_one_is_live() {
    let harness = Harness::start();
    let stats = harness.handle.link_stats();
    let state = harness.handle.link_state();

    let mut first = harness.device();
    assert!(state.wait_connected(PATIENCE));

    let mut second = FakeDevice::connect(harness.handle.device_addr()).unwrap();
    assert!(second.was_refused(PATIENCE).unwrap());
    assert!(wait_until(PATIENCE, || stats.refusals() == 1));

    // The live link is undisturbed by the refusal.
    assert!(state.is_connected());
    first.push(&[7.0]).unwrap();
    assert!(wait_until(PATIENCE, || stats.samples() == 1));

    first.disconnect();
}

#[test]
fn a_device_may_reconnect_after_dropping() {
    let harness = Harness::start();
    let stats = harness.handle.link_stats();
    let state = harness.handle.link_state();

    let first = harness.device();
    assert!(state.wait_connected(PATIENCE));
    first.disconnect();
    assert!(!state.wait_disconnected(PATIENCE));

    let second = harness.device();
    assert!(state.wait_connected(PATIENCE));
    assert!(wait_until(PATIENCE, || stats.connections() == 2));
    second.disconnect();
}

#[test]
fn scpi_keeps_working_while_a_device_is_streaming() {
    let harness = Harness::start();
    let mut uts = harness.client();
    let mut device = harness.device();

    device.push(&[0.5; 1024]).unwrap();
    assert_eq!(uts.query("SYST:DEV:CONN?").unwrap(), "1");

    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "50");

    device.disconnect();
}

#[test]
fn a_peer_outside_the_allow_list_is_turned_away() {
    let harness = Harness::builder()
        .project_json(
            r#"{
              "schemaVersion": 1,
              "name": "AllowList",
              "device": {
                "sampleRateHz": 1000.0,
                "unit": "velocity_um_s",
                "allowedPeers": ["10.99.99.99"]
              },
              "recording": { "durationSeconds": 0.05 }
            }"#,
        )
        .start();
    let stats = harness.handle.link_stats();

    let mut device = harness.device();
    assert!(device.was_refused(PATIENCE).unwrap());
    assert!(wait_until(PATIENCE, || stats.refusals() == 1));
    assert_eq!(stats.connections(), 0);
    assert!(!harness.handle.link_state().is_connected());
}
