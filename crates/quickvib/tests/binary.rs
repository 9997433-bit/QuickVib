//! The shipped executable, launched the way a UTS launches it (`docs/PLAN.md` 7.1, 13).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::PROJECT;
use quickvib_testkit::ScpiClient;

/// The executable built alongside this test.
fn quickvib() -> Command {
    Command::new(env!("CARGO_BIN_EXE_quickvib"))
}

/// A port nothing is listening on right now.
///
/// The command line takes `1..=65535`, so a test cannot ask the OS for an ephemeral port the
/// way the in-process suite does; it has to pick one and hand it over. Binding and releasing
/// is the standard way to find one, and the caller retries if the race is lost.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .expect("the OS must be able to assign a port")
}

/// Kills the child when the test ends, however it ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn help_and_version_print_and_exit_zero() {
    for flag in ["--help", "--version"] {
        let output = quickvib().arg(flag).output().unwrap();
        assert!(output.status.success(), "{flag} exited {:?}", output.status);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with("quickvib "), "{flag} printed: {text}");
    }
}

#[test]
fn a_bad_argument_exits_two_with_usage_on_stderr() {
    let output = quickvib().arg("--frobnicate").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("unknown option"), "{text}");
    assert!(text.contains("USAGE"), "{text}");
}

#[test]
fn an_out_of_range_port_exits_two() {
    let output = quickvib().args(["--scpi-port", "70000"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_missing_project_exits_four() {
    let dir = tempfile::tempdir().unwrap();
    let output = quickvib()
        .arg("--project")
        .arg(dir.path().join("nope.proj"))
        .args(["--scpi-port", &free_port().to_string()])
        .args(["--device-port", &free_port().to_string()])
        .arg("--no-auto-load")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn selecting_the_m300_backend_on_this_host_exits_two() {
    let output = quickvib()
        .args(["--backend", "m300"])
        .args(["--scpi-port", &free_port().to_string()])
        .args(["--device-port", &free_port().to_string()])
        .arg("--no-auto-load")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(
        text.contains("backend 'm300' is unavailable"),
        "stderr was: {text}"
    );
}

#[test]
fn a_launched_instrument_serves_a_real_uts_session() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("Test.proj");
    std::fs::write(&project, PROJECT).unwrap();

    // The banner reports the ports it actually bound, so the test reads them back rather
    // than assuming the request succeeded.
    let (mut running, banner) = loop {
        let mut child = quickvib()
            .arg("--project")
            .arg(&project)
            .args(["--scpi-port", &free_port().to_string()])
            .args(["--device-port", &free_port().to_string()])
            .arg("--no-auto-load")
            .env("QUICKVIB_STATE_DIR", dir.path().join("state"))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().expect("stdout was piped");
        let running = Running(child);
        // Structured log lines share stdout with the banner, so scan for the banner rather
        // than assuming it is first.
        let banner = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .take(50)
            .find(|line| line.contains("SCPI on port"));
        if let Some(banner) = banner {
            break (running, banner);
        }
        // Another process grabbed the port between the probe and the launch; try again.
        drop(running);
    };

    let scpi_port = field(&banner, "SCPI on port ");
    let device_port = field(&banner, "device on port ");
    assert!(scpi_port > 0 && device_port > 0, "banner: {banner}");
    assert!(banner.contains("backend mock"), "banner: {banner}");

    let mut uts =
        ScpiClient::connect_with_timeout(("127.0.0.1", scpi_port), Duration::from_secs(20))
            .unwrap();

    assert_eq!(
        uts.query("*IDN?").unwrap(),
        "QuickVib,M300-SCPI,SN-0001,1.0.0"
    );
    uts.command("CONF:REC:DUR 0.05").unwrap();
    uts.command("INIT").unwrap();
    assert_eq!(uts.query("REC:WAIT?").unwrap(), "1");
    assert_eq!(uts.query("TRAC:POIN?").unwrap(), "50");

    let export = dir.path().join("run001.csv");
    uts.command(&format!("MMEM:STOR:TRAC \"{}\"", export.display()))
        .unwrap();
    assert_eq!(uts.error().unwrap(), "0,\"No error\"");
    assert!(export.is_file());

    // The device port is bound too, so an M300 could dial in.
    let device = std::net::TcpStream::connect(("127.0.0.1", device_port)).unwrap();
    drop(device);

    uts.close();
    let _ = running.0.kill();
}

/// Pull a port number out of the banner line.
fn field(banner: &str, prefix: &str) -> u16 {
    banner
        .split(prefix)
        .nth(1)
        .and_then(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .unwrap_or(0)
}
