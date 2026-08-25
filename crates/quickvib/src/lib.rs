//! QuickVib: a SCPI-over-TCP front end for the M300 laser Doppler vibrometer.
//!
//! The executable is a thin shell over this library so the integration suite can start a real
//! instrument in-process, on ephemeral ports, and drive it over real loopback sockets — the
//! code under test is then the code that ships (`docs/PLAN.md` 17.2).
//!
//! ```no_run
//! use quickvib::{cli::Options, AppBuilder};
//!
//! let app = AppBuilder::new(Options::default()).build()?;
//! println!("{}", app.banner()?);
//! app.run();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]

pub mod app;
pub mod backend_factory;
pub mod cli;
pub mod device_server;
#[cfg(feature = "gui")]
pub mod gui;
pub mod log;
pub mod scpi_server;

pub use app::{
    build, App, AppBuilder, AppHandle, StartupError, EXIT_BACKEND_FAILURE, EXIT_BAD_ARGUMENTS,
    EXIT_BIND_FAILURE, EXIT_OK, EXIT_PROJECT_FAILURE,
};
pub use backend_factory::BackendError;
pub use cli::{parse, CliError, CliOutcome, Options};
pub use device_server::DeviceServer;
pub use log::LineLogger;
pub use scpi_server::ScpiServer;
