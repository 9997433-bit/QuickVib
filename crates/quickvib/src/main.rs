//! The QuickVib executable.
//!
//! Parse the command line, build the instrument, print one banner, then block in the accept
//! loops until the process is terminated. Ctrl-C is left to the operating system on purpose
//! (`docs/PLAN.md` 7.1): a graceful drain would need either an `unsafe` console-control shim
//! or a dependency the zero-dependency stance excludes, and the normal shutdown path for a
//! test executive is `ABOR` followed by closing the socket.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::Write;

use quickvib::cli::{self, CliOutcome};
use quickvib::{AppBuilder, EXIT_BAD_ARGUMENTS, EXIT_OK};

fn main() {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();

    let options = match cli::parse(&arguments) {
        Ok(CliOutcome::Print(text)) => {
            print!("{text}");
            exit(EXIT_OK);
        }
        Ok(CliOutcome::Run(options)) => *options,
        Err(error) => {
            eprintln!("quickvib: {error}");
            eprint!("\n{}", cli::usage());
            exit(EXIT_BAD_ARGUMENTS);
        }
    };

    let headless = options.headless;
    let app = match AppBuilder::new(options).build() {
        Ok(app) => app,
        Err(error) => {
            eprintln!("quickvib: {error}");
            exit(error.exit_code());
        }
    };

    if !headless {
        match app.banner() {
            Ok(banner) => println!("{banner}"),
            Err(error) => eprintln!("quickvib: {error}"),
        }
    }

    app.run();
    exit(EXIT_OK);
}

/// Flush both streams before leaving, since `std::process::exit` runs no destructors.
fn exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}
