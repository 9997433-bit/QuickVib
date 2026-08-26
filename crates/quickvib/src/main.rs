//! The QuickVib executable.
//!
//! Parse the command line, build the instrument, print one banner, then block in the accept
//! loops until the process is terminated. Ctrl-C is left to the operating system on purpose
//! (`docs/PLAN.md` 7.1): a graceful drain would need either an `unsafe` console-control shim
//! or a dependency the zero-dependency stance excludes, and the normal shutdown path for a
//! test executive is `ABOR` followed by closing the socket.
//!
//! `--headless` is the UTS-facing mode and never opens a window. Without it, a build carrying
//! the `gui` feature moves both accept loops onto background threads and puts the desktop
//! window on the main thread; a build without the feature — the default, and what CI
//! cross-compiles — serves from the console exactly as it always has.

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

    if headless {
        app.run();
    } else {
        serve_with_window(app);
    }
    exit(EXIT_OK);
}

/// The interactive path: the window when this build has one and the display allows it,
/// otherwise the same foreground accept loops as `--headless`.
#[cfg(feature = "gui")]
fn serve_with_window(app: quickvib::App) {
    if let quickvib::gui::WindowOutcome::Unavailable { reason, handle } = quickvib::gui::run(app) {
        eprintln!("quickvib: {reason}");
        eprintln!("quickvib: serving from the console; use --headless to skip the window");
        handle.wait();
    }
}

#[cfg(not(feature = "gui"))]
fn serve_with_window(app: quickvib::App) {
    // Built without the `gui` feature: there is no window to open, so the interactive mode is
    // the banner that has already been printed plus the ordinary accept loops.
    app.run();
}

/// Flush both streams before leaving, since `std::process::exit` runs no destructors.
fn exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}
