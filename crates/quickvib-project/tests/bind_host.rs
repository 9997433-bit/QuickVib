//! Regression coverage for `server.bindHost` and its compatibility default.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use quickvib_project::Project;

fn project_with(server: &str) -> Project {
    Project::from_json_str(&format!(
        r#"{{
            "schemaVersion": 1,
            "name": "BindHost",
            "device": {{ "sampleRateHz": 1000.0, "unit": "velocity_um_s" }},
            "recording": {{ "durationSeconds": 0.01 }}
            {server}
        }}"#
    ))
    .expect("the project must parse")
}

#[test]
fn omitted_bind_host_defaults_to_all_ipv4_interfaces() {
    let project = project_with("");
    assert_eq!(project.server.bind_host.to_string(), "0.0.0.0");
}

#[test]
fn camel_case_bind_host_parses_and_round_trips() {
    let project = project_with(r#", "server": { "bindHost": "127.0.0.1" }"#);
    assert_eq!(project.server.bind_host.to_string(), "127.0.0.1");

    let serialized = project.to_json_string().unwrap();
    assert!(serialized.contains(r#""bindHost": "127.0.0.1""#));
    let round_tripped = Project::from_json_str(&serialized).unwrap();
    assert_eq!(round_tripped.server.bind_host.to_string(), "127.0.0.1");
}
