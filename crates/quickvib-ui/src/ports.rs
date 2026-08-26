//! Live ports versus project ports.
//!
//! QuickVib binds its two listeners once, at startup, from `--scpi-port`/`--device-port` if
//! they were given and from the project file otherwise. The project file can then be edited,
//! and a saved port is a port *the next* launch will use — the sockets this process is holding
//! do not move. A window that draws the project's `5025` next to the word "port" while the
//! process is answering on `15025` is telling an operator something false, so the two are kept
//! apart here and drawn apart in the window: the bound ports are read-only fact, the project
//! ports are an editable intention, and a disagreement between them is called out.
//!
//! None of this needs a display, so all of it is tested in the ordinary `cargo test` run.

use std::net::SocketAddr;

/// The addresses this process actually bound, as reported by the listeners themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LivePorts {
    /// The bound SCPI address, when a listener was started.
    pub scpi: Option<SocketAddr>,
    /// The bound device address, when a listener was started.
    pub device: Option<SocketAddr>,
}

impl LivePorts {
    /// Both bound addresses.
    #[must_use]
    pub const fn new(scpi: SocketAddr, device: SocketAddr) -> Self {
        Self {
            scpi: Some(scpi),
            device: Some(device),
        }
    }

    /// The SCPI port number, when a listener was started.
    #[must_use]
    pub fn scpi_port(&self) -> Option<u16> {
        self.scpi.map(|addr| addr.port())
    }

    /// The device port number, when a listener was started.
    #[must_use]
    pub fn device_port(&self) -> Option<u16> {
        self.device.map(|addr| addr.port())
    }

    /// The SCPI port as text, or `-` when nothing is bound.
    #[must_use]
    pub fn scpi_text(&self) -> String {
        port_text(self.scpi_port())
    }

    /// The device port as text, or `-` when nothing is bound.
    #[must_use]
    pub fn device_text(&self) -> String {
        port_text(self.device_port())
    }
}

/// A port number as the window shows it, or `-` when no listener was started.
#[must_use]
pub fn port_text(port: Option<u16>) -> String {
    port.map_or_else(|| "-".to_owned(), |port| port.to_string())
}

/// One row of the I/O section: what is bound now against what the project asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRow {
    /// The port the process is listening on, when a listener was started.
    pub live: Option<u16>,
    /// The port the form currently holds, when its text parses as a port.
    pub project: Option<u16>,
}

impl PortRow {
    /// Compare a bound port with the port text in the form.
    ///
    /// Unparsable text is not a disagreement: the field is already reporting its own error,
    /// and one bad keystroke must not raise a restart warning on top of it.
    #[must_use]
    pub fn new(live: Option<u16>, project_text: &str) -> Self {
        Self {
            live,
            project: project_text.trim().parse::<u16>().ok().filter(|p| *p != 0),
        }
    }

    /// Whether the project asks for a port other than the one that is bound — the case where
    /// the form's number is real but is not what this process is answering on.
    #[must_use]
    pub fn differs(&self) -> bool {
        match (self.live, self.project) {
            (Some(live), Some(project)) => live != project,
            _ => false,
        }
    }
}

/// Whether either listener disagrees with the project file, i.e. whether the window has to
/// warn that the bound ports are not the ones being edited.
#[must_use]
pub fn any_mismatch(rows: &[PortRow]) -> bool {
    rows.iter().any(PortRow::differs)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn live_ports_report_what_was_bound() {
        let live = LivePorts::new(addr(15_025), addr(19_123));
        assert_eq!(live.scpi_port(), Some(15_025));
        assert_eq!(live.device_port(), Some(19_123));
        assert_eq!(live.scpi_text(), "15025");
        assert_eq!(live.device_text(), "19123");
    }

    #[test]
    fn an_unbound_listener_shows_a_dash() {
        let live = LivePorts::default();
        assert_eq!(live.scpi_text(), "-");
        assert_eq!(live.device_text(), "-");
        assert_eq!(port_text(None), "-");
    }

    #[test]
    fn a_cli_override_is_reported_as_a_mismatch() {
        // The exact bug this module exists for: the project says 5025/9123, the process is
        // answering on 15025/19123 because the command line said so.
        let rows = [
            PortRow::new(Some(15_025), "5025"),
            PortRow::new(Some(19_123), "9123"),
        ];
        assert!(rows[0].differs());
        assert!(rows[1].differs());
        assert!(any_mismatch(&rows));
    }

    #[test]
    fn matching_ports_raise_no_warning() {
        let rows = [
            PortRow::new(Some(5025), "5025"),
            PortRow::new(Some(9123), " 9123 "),
        ];
        assert!(!any_mismatch(&rows));
    }

    #[test]
    fn a_half_typed_port_is_not_a_mismatch() {
        for text in ["", "  ", "50", "not a port", "70000", "0"] {
            let row = PortRow::new(Some(5025), text);
            if text == "50" {
                assert!(row.differs(), "50 parses and really is a different port");
            } else {
                assert!(!row.differs(), "{text:?} must not raise a restart warning");
            }
        }
    }

    #[test]
    fn nothing_bound_means_nothing_to_disagree_with() {
        assert!(!PortRow::new(None, "5025").differs());
    }
}
