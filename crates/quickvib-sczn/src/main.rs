//! The `m300-device-sim` executable: a thin shell over [`quickvib_sczn`].
//!
//! Everything interesting — the protocol, the state machine, the streaming loop — is library
//! code so it can be unit-tested and driven in-process by the integration suite. This file only
//! maps the result onto stdio and an exit code.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use quickvib_core::CancelToken;
use quickvib_sczn::cli::{parse, usage, SimOutcome};
use quickvib_sczn::runner::run;

/// Exit code for a run that got at least one link.
const EXIT_OK: u8 = 0;
/// Exit code for a bad command line.
const EXIT_BAD_ARGUMENTS: u8 = 2;
/// Exit code for a run that ended without ever connecting.
const EXIT_CONNECT_FAILURE: u8 = 3;

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();

    let options = match parse(&arguments) {
        Ok(SimOutcome::Run(options)) => options,
        Ok(SimOutcome::Print(text)) => {
            print!("{text}");
            return ExitCode::from(EXIT_OK);
        }
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "m300-device-sim: {error}\n\n{}", usage());
            return ExitCode::from(EXIT_BAD_ARGUMENTS);
        }
    };

    let summary = run(&options, &CancelToken::new());
    let stop = summary
        .stop
        .map_or_else(|| "unknown".to_owned(), |reason| reason.to_string());

    if summary.connected() {
        println!(
            "m300-device-sim: {} session(s) with {}:{}, answered {} commands, \
             sent {} samples in {} blocks, stopped: {stop}",
            summary.sessions,
            options.host,
            options.port,
            summary.requests_handled,
            summary.samples_sent,
            summary.blocks_sent
        );
        ExitCode::from(EXIT_OK)
    } else {
        let _ = writeln!(
            std::io::stderr(),
            "m300-device-sim: could not connect to {}:{} after {} attempt(s), stopped: {stop}",
            options.host,
            options.port,
            summary.connect_failures
        );
        ExitCode::from(EXIT_CONNECT_FAILURE)
    }
}
