//! Regression coverage for the public bind-host configuration surfaces.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use quickvib::cli::{parse, usage, CliError, CliOutcome, Options};
use quickvib::{App, AppBuilder};
use quickvib_core::{NullLogger, TestClock};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const WILDCARD: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

fn parsed(arguments: &[&str]) -> Options {
    let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    match parse(&arguments).expect("the command line must parse") {
        CliOutcome::Run(options) => *options,
        CliOutcome::Print(text) => panic!("expected run options, got: {text}"),
    }
}

fn build_on_ephemeral_ports(mut options: Options) -> App {
    // Port zero is deliberately an integration-test-only request to the OS. The CLI rejects
    // it, so overwrite only the ports after exercising the real argument parser.
    options.scpi_port = Some(0);
    options.device_port = Some(0);
    options.no_auto_load = true;

    AppBuilder::new(options)
        .clock(Arc::new(TestClock::at_epoch()))
        .logger(Arc::new(NullLogger))
        .last_project(None)
        .build()
        .expect("both listeners must bind")
}

fn assert_both_listeners_use(app: &App, expected: IpAddr) {
    assert_eq!(app.scpi_addr().unwrap().ip(), expected);
    assert_eq!(app.device_addr().unwrap().ip(), expected);
}

#[test]
fn default_bind_host_remains_all_ipv4_interfaces() {
    let app = build_on_ephemeral_ports(Options::default());
    assert_both_listeners_use(&app, WILDCARD);
}

#[test]
fn separated_bind_option_controls_both_listeners() {
    let app = build_on_ephemeral_ports(parsed(&["--bind", "127.0.0.1"]));
    assert_both_listeners_use(&app, LOOPBACK);
}

#[test]
fn inline_bind_option_controls_both_listeners() {
    let app = build_on_ephemeral_ports(parsed(&["--bind=127.0.0.1"]));
    assert_both_listeners_use(&app, LOOPBACK);
}

#[test]
fn bind_option_requires_a_value_and_is_documented() {
    for arguments in [["--bind"].as_slice(), ["--bind="].as_slice()] {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        assert_eq!(
            parse(&arguments),
            Err(CliError::MissingValue("--bind")),
            "for {arguments:?}"
        );
    }
    assert!(usage().contains("--bind"));
}

#[test]
fn project_bind_host_controls_both_listeners_without_a_cli_override() {
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("Loopback.proj");
    std::fs::write(
        &project_path,
        r#"{
            "schemaVersion": 1,
            "name": "Loopback",
            "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
            "recording": { "durationSeconds": 0.01 },
            "server": { "bindHost": "127.0.0.1" }
        }"#,
    )
    .unwrap();

    let app = build_on_ephemeral_ports(Options {
        project: Some(project_path),
        ..Options::default()
    });
    assert_both_listeners_use(&app, LOOPBACK);
}

#[test]
fn command_line_bind_overrides_the_project_bind_host() {
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("Wildcard.proj");
    std::fs::write(
        &project_path,
        r#"{
            "schemaVersion": 1,
            "name": "Wildcard",
            "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
            "recording": { "durationSeconds": 0.01 },
            "server": { "bindHost": "0.0.0.0" }
        }"#,
    )
    .unwrap();

    let project = project_path.to_string_lossy().into_owned();
    let app = build_on_ephemeral_ports(parsed(&["--project", &project, "--bind", "127.0.0.1"]));
    assert_both_listeners_use(&app, LOOPBACK);
}
