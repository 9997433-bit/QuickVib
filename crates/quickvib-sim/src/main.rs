//! The `m300-sim` executable: a thin shell over [`quickvib_sim`].
//!
//! Everything interesting — argument parsing, the waveform, the streaming loop — is library
//! code so it can be unit-tested and driven in-process by the integration suite. This file
//! only maps the result onto stdio and an exit code.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use quickvib_core::CancelToken;
use quickvib_sim::{parse, run, usage, SimOutcome};

/// Exit code for a clean stop: the peer closed, or `--duration` elapsed.
const EXIT_OK: u8 = 0;
/// Exit code for a bad command line.
const EXIT_BAD_ARGUMENTS: u8 = 2;
/// Exit code for a device port nothing is listening on.
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
            let _ = writeln!(std::io::stderr(), "m300-sim: {error}\n\n{}", usage());
            return ExitCode::from(EXIT_BAD_ARGUMENTS);
        }
    };

    let cancel = CancelToken::new();
    match run(&options, &cancel) {
        Ok(summary) => {
            println!(
                "m300-sim: sent {} samples to {}:{} at {} S/s, stopped: {}",
                summary.samples_sent, options.host, options.port, options.rate_hz, summary.stop
            );
            ExitCode::from(EXIT_OK)
        }
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "m300-sim: could not connect to {}:{}: {error}",
                options.host,
                options.port
            );
            ExitCode::from(EXIT_CONNECT_FAILURE)
        }
    }
}
