//! What the window knows before it draws anything: the ports this process bound, the backend
//! it opened, and the version it is.
//!
//! Kept out of the `gui` feature so the window title and the live-port plumbing are compiled
//! and tested on a machine with no display.

use quickvib_core::BackendKind;

use crate::i18n::{self, Label, Lang};
use crate::ports::LivePorts;

/// Everything the window shows that is fixed for the life of the process.
#[derive(Debug, Clone, Default)]
pub struct GuiOptions {
    /// The addresses the two listeners really bound — never the project's copy of them.
    pub live: LivePorts,
    /// The backend the process opened at startup.
    pub backend: BackendKind,
    /// The application version, shown in the title bar and the status bar.
    pub version: String,
}

impl GuiOptions {
    /// The window title: version, both live ports and the backend, so a screenshot of the
    /// window is enough to tell two instances apart — and enough to see which ports are
    /// really answering.
    #[must_use]
    pub fn title(&self, lang: Lang) -> String {
        format!(
            "QuickVib {} · SCPI {} · {} {} · {}",
            self.version,
            self.live.scpi_text(),
            lang.t(Label::DeviceShort),
            self.live.device_text(),
            i18n::backend_label(lang, self.backend)
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    use std::net::SocketAddr;

    fn options() -> GuiOptions {
        GuiOptions {
            live: LivePorts::new(
                SocketAddr::from(([127, 0, 0, 1], 15_025)),
                SocketAddr::from(([127, 0, 0, 1], 19_123)),
            ),
            backend: BackendKind::Mock,
            version: "1.0.0".to_owned(),
        }
    }

    #[test]
    fn the_title_is_chinese_and_names_the_live_ports() {
        assert_eq!(
            options().title(Lang::Zh),
            "QuickVib 1.0.0 · SCPI 15025 · 设备 19123 · 模拟"
        );
    }

    #[test]
    fn the_title_follows_the_language_switch() {
        assert_eq!(
            options().title(Lang::En),
            "QuickVib 1.0.0 · SCPI 15025 · Device 19123 · Simulated"
        );
    }

    #[test]
    fn a_process_with_no_listeners_still_has_a_title() {
        let options = GuiOptions {
            version: "1.0.0".to_owned(),
            ..GuiOptions::default()
        };
        assert!(options.title(Lang::Zh).contains("SCPI -"));
    }
}
