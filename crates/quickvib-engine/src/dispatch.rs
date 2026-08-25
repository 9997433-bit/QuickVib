//! Turning a parsed [`Command`] into a [`Response`].
//!
//! The match is exhaustive over [`Command`], so adding a variant to the SCPI surface without
//! wiring it up here is a compile error rather than a silently unimplemented command.

use std::sync::Arc;

use quickvib_core::ScpiError;
use quickvib_measure::format_fixed;
use quickvib_scpi::{Command, Response};

use crate::engine::Engine;

/// Execute one command against the engine.
///
/// Failures never produce a response line: they push a SCPI-99 error onto the queue, which the
/// UTS collects with `SYST:ERR?`. That is standard instrument behaviour and it keeps queries
/// and commands consistent.
#[must_use]
pub fn dispatch(engine: &Arc<Engine>, command: &Command) -> Response {
    match command {
        // ---- IEEE 488.2 ----
        Command::Idn => Response::Text(engine.identity()),
        Command::Rst => {
            engine.reset();
            Response::None
        }
        Command::Cls => {
            engine.clear_status();
            Response::None
        }
        Command::Opc => {
            engine.request_opc();
            Response::None
        }
        Command::OpcQuery => {
            engine.wait_operation_complete();
            Response::Text("1".to_owned())
        }
        Command::SystemErrorQuery => Response::Text(engine.pop_error().wire_form()),

        // ---- Project / mass memory ----
        Command::MemoryLoadState(path) => {
            report(engine, engine.load_project(path));
            Response::None
        }
        Command::MemoryStoreState(path) => {
            report(engine, engine.store_project(path));
            Response::None
        }
        Command::MemoryLoadAuto => {
            report(engine, engine.load_auto());
            Response::None
        }
        Command::MemoryLoadAutoQuery => Response::Text(match engine.auto_load_path() {
            Some(path) => format!("\"{}\"", path.display()),
            None => "\"\"".to_owned(),
        }),
        Command::MemoryStoreTrace(path) => {
            report(engine, engine.export_capture(path));
            Response::None
        }

        // ---- Configuration ----
        Command::ConfigureDuration(seconds) => {
            report(engine, engine.set_duration(*seconds));
            Response::None
        }
        Command::ConfigureDurationQuery => {
            Response::Text(format!("{:.3}", engine.duration_seconds()))
        }
        Command::Format(format) => {
            report(engine, engine.set_format(*format));
            Response::None
        }
        Command::FormatQuery => Response::Text(engine.format().as_scpi_str().to_owned()),

        // ---- Recording control ----
        Command::Initiate => {
            report(engine, engine.start_recording());
            Response::None
        }
        Command::Abort => {
            engine.abort();
            Response::None
        }
        Command::RecordStateQuery => Response::Text(engine.state().as_scpi_str().to_owned()),
        Command::RecordWaitQuery => {
            Response::Text(if engine.wait_for_run() { "1" } else { "0" }.to_owned())
        }

        // ---- Data retrieval ----
        Command::Fetch => match engine.capture() {
            Ok(samples) => Response::Samples(samples),
            Err(error) => {
                engine.push_error(error);
                Response::None
            }
        },
        Command::TracePointsQuery => match engine.capture() {
            Ok(samples) => Response::Text(samples.len().to_string()),
            Err(error) => {
                engine.push_error(error);
                Response::None
            }
        },

        // ---- Calculated measurements ----
        Command::CalculatePeak => scalar(engine, |m| m.peak),
        Command::CalculateRms => scalar(engine, |m| m.rms),
        Command::CalculatePeakToPeak => scalar(engine, |m| m.peak_to_peak),
        Command::CalculateAll => match engine.measurements() {
            Ok(measurements) => {
                Response::Text(measurements.all_wire_form(engine.response_decimals()))
            }
            Err(error) => {
                engine.push_error(error);
                Response::None
            }
        },

        // ---- System / device ----
        Command::SystemDeviceConnectedQuery => {
            Response::Text(if engine.device_connected() { "1" } else { "0" }.to_owned())
        }
        Command::SystemVersionQuery => Response::Text(engine.scpi_version().to_owned()),
    }
}

/// Push `result`'s error, if any, onto the queue.
fn report<T>(engine: &Arc<Engine>, result: Result<T, ScpiError>) {
    if let Err(error) = result {
        engine.push_error(error);
    }
}

fn scalar(
    engine: &Arc<Engine>,
    select: impl Fn(&quickvib_measure::MeasurementSet) -> f64,
) -> Response {
    match engine.measurements() {
        Ok(measurements) => Response::Text(format_fixed(
            select(&measurements),
            engine.response_decimals(),
        )),
        Err(error) => {
            engine.push_error(error);
            Response::None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::engine::{Engine, EngineConfig};
    use quickvib_core::{Clock, ExportFormat, NullLogger, SampleUnit, TestClock};
    use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend};
    use quickvib_project::Project;
    use quickvib_scpi::parse_line;

    const PROJECT: &str = r#"{
        "schemaVersion": 1,
        "name": "DispatchTest",
        "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
        "recording": { "durationSeconds": 0.02 },
        "mock": { "signal": { "components": [
            { "frequencyHz": 50.0, "amplitude": 3.0, "phaseDeg": 0.0 } ], "seed": 5 } }
    }"#;

    fn engine() -> Arc<Engine> {
        let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
        let mut backend = MockBackend::new(Arc::clone(&clock));
        backend
            .open(&DeviceOpenOptions::new(
                1000.0,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();
        let engine = Engine::new(EngineConfig {
            clock,
            logger: Arc::new(NullLogger),
            backend: Box::new(backend),
            last_project: None,
            version: "1.0.0-test".to_owned(),
        });
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine
    }

    /// Run a full line through the parser and the dispatcher, returning the response text.
    fn send(engine: &Arc<Engine>, line: &str) -> String {
        let commands = parse_line(line.as_bytes()).unwrap();
        let responses: Vec<Response> = commands.iter().map(|c| dispatch(engine, c)).collect();
        let mut out = Vec::new();
        quickvib_scpi::write_response_line(&mut out, &responses).unwrap();
        String::from_utf8(out).unwrap().trim_end().to_owned()
    }

    #[test]
    fn idn_reports_four_fields() {
        let engine = engine();
        assert_eq!(send(&engine, "*IDN?").split(',').count(), 4);
    }

    #[test]
    fn commands_produce_no_response_line() {
        let engine = engine();
        assert_eq!(send(&engine, "*CLS"), "");
        assert_eq!(send(&engine, "ABOR"), "");
    }

    #[test]
    fn duration_is_reported_with_three_decimals() {
        let engine = engine();
        assert_eq!(send(&engine, "CONF:REC:DUR?"), "0.020");
        assert_eq!(send(&engine, "CONF:REC:DUR 1.5"), "");
        assert_eq!(send(&engine, "CONF:REC:DUR?"), "1.500");
    }

    #[test]
    fn an_out_of_range_duration_queues_222() {
        let engine = engine();
        assert_eq!(send(&engine, "CONF:REC:DUR 0"), "");
        assert_eq!(send(&engine, "SYST:ERR?"), "-222,\"Data out of range\"");
        assert_eq!(send(&engine, "SYST:ERR?"), "0,\"No error\"");
    }

    #[test]
    fn format_selection_round_trips() {
        let engine = engine();
        assert_eq!(send(&engine, "FORM?"), "CSV");
        send(&engine, "FORM TXT");
        assert_eq!(send(&engine, "FORM?"), "TXT");
        assert_eq!(engine.format(), ExportFormat::Txt);
    }

    #[test]
    fn the_full_happy_path_works_over_dispatch() {
        let engine = engine();
        assert_eq!(send(&engine, "SYST:DEV:CONN?"), "1");
        assert_eq!(send(&engine, "REC:STAT?"), "IDLE");
        send(&engine, "INIT");
        assert_eq!(send(&engine, "REC:WAIT?"), "1");
        assert_eq!(send(&engine, "REC:STAT?"), "COMPLETE");
        assert_eq!(send(&engine, "TRAC:POIN?"), "20");

        let all = send(&engine, "CALC:MEAS:ALL?");
        let fields: Vec<&str> = all.split(',').collect();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0], send(&engine, "CALC:MEAS:PEAK?"));
        assert_eq!(fields[1], send(&engine, "CALC:MEAS:RMS?"));
        assert_eq!(fields[2], send(&engine, "CALC:MEAS:PP?"));

        let fetched = send(&engine, "FETC?");
        assert_eq!(fetched.split(',').count(), 20);
        assert_eq!(send(&engine, "TRAC:DATA?"), fetched);
    }

    #[test]
    fn data_queries_before_a_run_queue_230() {
        let engine = engine();
        for query in [
            "FETC?",
            "TRAC:DATA?",
            "TRAC:POIN?",
            "CALC:MEAS:PEAK?",
            "CALC:MEAS:ALL?",
        ] {
            assert_eq!(send(&engine, query), "", "for {query}");
            assert_eq!(
                send(&engine, "SYST:ERR?"),
                "-230,\"Data corrupt or stale\"",
                "for {query}"
            );
        }
    }

    #[test]
    fn applying_a_project_stops_the_state_from_claiming_complete() {
        let engine = engine();
        send(&engine, "INIT");
        assert_eq!(send(&engine, "REC:WAIT?"), "1");
        assert_eq!(send(&engine, "REC:STAT?"), "COMPLETE");

        // What the GUI's Apply button does.
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();

        assert_eq!(send(&engine, "REC:STAT?"), "IDLE");
        assert_eq!(send(&engine, "REC:WAIT?"), "0");
        for query in ["FETC?", "TRAC:POIN?", "CALC:MEAS:ALL?"] {
            assert_eq!(send(&engine, query), "", "for {query}");
            assert_eq!(
                send(&engine, "SYST:ERR?"),
                "-230,\"Data corrupt or stale\"",
                "for {query}"
            );
        }
    }

    #[test]
    fn measurement_decimals_follow_the_project() {
        let engine = engine();
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.measurement.response_decimals = 2;
        engine.adopt_project(project, None).unwrap();
        send(&engine, "INIT");
        send(&engine, "REC:WAIT?");
        let peak = send(&engine, "CALC:MEAS:PEAK?");
        assert_eq!(peak.split('.').nth(1).unwrap().len(), 2, "{peak}");
    }

    #[test]
    fn compound_queries_are_joined_with_semicolons() {
        let engine = engine();
        let response = send(&engine, "*CLS;FORM?;REC:STAT?");
        assert_eq!(response, "CSV;IDLE");
    }

    #[test]
    fn auto_load_query_reports_an_empty_string_without_a_record() {
        let engine = engine();
        assert_eq!(send(&engine, "MMEM:LOAD:AUTO?"), "\"\"");
    }

    #[test]
    fn syst_vers_reports_the_scpi_standard() {
        let engine = engine();
        assert_eq!(send(&engine, "SYST:VERS?"), "1999.0");
    }

    #[test]
    fn opc_query_returns_one_after_a_run() {
        let engine = engine();
        send(&engine, "INIT");
        assert_eq!(send(&engine, "*OPC?"), "1");
    }

    #[test]
    fn store_trace_writes_the_capture() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine();
        send(&engine, "INIT");
        send(&engine, "REC:WAIT?");
        let path = dir.path().join("run001.csv");
        send(&engine, &format!("MMEM:STOR:TRAC \"{}\"", path.display()));
        assert_eq!(send(&engine, "SYST:ERR?"), "0,\"No error\"");
        assert!(path.is_file());
    }

    #[test]
    fn loading_a_missing_project_queues_256() {
        let engine = engine();
        send(&engine, "MMEM:LOAD:STAT \"definitely-not-here.proj\"");
        assert_eq!(send(&engine, "SYST:ERR?"), "-256,\"File name not found\"");
    }
}
