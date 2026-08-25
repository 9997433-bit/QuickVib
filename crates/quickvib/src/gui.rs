//! Starting the desktop window over a running instrument.
//!
//! The split of responsibilities is the point of this module. Both accept loops move onto
//! background threads, the window takes the main thread — which every desktop windowing
//! system insists on — and the two halves meet at the one `Arc<Engine>` they share. A field
//! edited in the window and a `CONF:REC:DUR` arriving on port 5025 therefore land in exactly
//! the same instrument model, and closing the window cancels both loops on the way out.

use std::sync::Arc;

use quickvib_ui::{window::GuiOptions, UiController};

use crate::app::App;

/// Run the window in the foreground with the servers behind it.
///
/// Returns once the operator closes the window, having stopped both accept loops.
///
/// # Errors
/// The message from the windowing system when no window could be created — a headless
/// session, no compositor, no GPU. The servers are left running and the returned
/// [`crate::AppHandle`] is handed back so the caller can keep serving from the console.
pub fn run(app: App) -> Result<(), (String, crate::AppHandle)> {
    let backend = app.backend();
    let handle = app.spawn();

    let options = GuiOptions {
        scpi_addr: Some(handle.scpi_addr()),
        device_addr: Some(handle.device_addr()),
        backend,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let controller =
        UiController::new(Arc::clone(handle.engine())).with_link(Arc::clone(handle.link_state()));

    match quickvib_ui::run(controller, options) {
        Ok(()) => {
            handle.shutdown();
            Ok(())
        }
        Err(error) => Err((error, handle)),
    }
}
