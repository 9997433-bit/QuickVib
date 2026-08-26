//! Starting the desktop window over a running instrument.
//!
//! The split of responsibilities is the point of this module. Both accept loops move onto
//! background threads, the window takes the main thread — which every desktop windowing
//! system insists on — and the two halves meet at the one `Arc<Engine>` they share. A field
//! edited in the window and a `CONF:REC:DUR` arriving on port 5025 therefore land in exactly
//! the same instrument model, and closing the window cancels both loops on the way out.

use std::net::SocketAddr;
use std::sync::Arc;

use quickvib_ui::{GuiOptions, LivePorts, UiController};

use crate::app::App;

/// How the windowed run ended.
#[derive(Debug)]
pub enum WindowOutcome {
    /// The operator closed the window; both accept loops have been stopped.
    Closed,
    /// No window could be created — a headless session, no compositor, no GPU. The servers
    /// are still running behind the returned handle, so the caller can go on serving the UTS
    /// from the console instead of exiting.
    Unavailable {
        /// What the windowing system said.
        reason: String,
        /// The still-running instrument.
        handle: crate::AppHandle,
    },
}

/// Run the window in the foreground with the servers behind it.
///
/// Returns once the operator closes the window, or immediately with
/// [`WindowOutcome::Unavailable`] when this host has no display to put one on.
#[must_use]
pub fn run(app: App) -> WindowOutcome {
    let backend = app.backend();
    let handle = app.spawn();

    // The window is given the addresses the listeners actually bound, never the project
    // file's copy of them: with `--scpi-port`/`--device-port` in play, or with port 0 asking
    // the OS to choose, the two are different numbers and only these are answering.
    let options = GuiOptions {
        live: LivePorts {
            scpi: bound(handle.scpi_addr()),
            device: bound(handle.device_addr()),
        },
        backend,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let controller =
        UiController::new(Arc::clone(handle.engine())).with_link(Arc::clone(handle.link_state()));

    match quickvib_ui::run(controller, options) {
        Ok(()) => {
            handle.shutdown();
            WindowOutcome::Closed
        }
        Err(reason) => WindowOutcome::Unavailable { reason, handle },
    }
}

/// A listener whose address could not be read reports port 0; that is "nothing is bound"
/// rather than "bound to port zero", and the window shows it as a dash.
fn bound(addr: SocketAddr) -> Option<SocketAddr> {
    (addr.port() != 0).then_some(addr)
}
