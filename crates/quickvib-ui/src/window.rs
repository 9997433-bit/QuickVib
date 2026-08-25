//! The egui/eframe window.
//!
//! Compiled only with the `gui` feature. It owns no rules: every control reads and writes the
//! [`ProjectForm`], every action goes through [`UiController`], and everything it displays
//! comes from one [`crate::StatusSnapshot`] taken at the top of the frame. That division is
//! what lets the interesting behaviour be tested without a display.
//!
//! The window runs on the main thread — a hard requirement of every desktop windowing system —
//! while the SCPI and device accept loops run on threads behind it, so the instrument keeps
//! answering the UTS while an operator is clicking around.

use std::net::SocketAddr;
use std::time::Duration;

use eframe::egui;
use quickvib_core::{BackendKind, ExportFormat, SampleUnit};

use crate::controller::UiController;
use crate::status::{Notice, StatusSnapshot};

/// Everything the window shows that is fixed for the life of the process.
#[derive(Debug, Clone, Default)]
pub struct GuiOptions {
    /// The bound SCPI address, when a listener was started.
    pub scpi_addr: Option<SocketAddr>,
    /// The bound device address, when a listener was started.
    pub device_addr: Option<SocketAddr>,
    /// The backend the process opened at startup.
    pub backend: BackendKind,
    /// The application version, shown in the title bar.
    pub version: String,
}

impl GuiOptions {
    /// The window title: version and both ports, so a screenshot of the window is enough to
    /// tell two instances apart.
    #[must_use]
    pub fn title(&self) -> String {
        format!(
            "QuickVib {} - SCPI {} - device {} - backend {}",
            self.version,
            port_text(self.scpi_addr),
            port_text(self.device_addr),
            self.backend
        )
    }
}

fn port_text(addr: Option<SocketAddr>) -> String {
    addr.map_or_else(|| "-".to_owned(), |addr| addr.port().to_string())
}

/// Open the window and run the event loop until the operator closes it.
///
/// Blocks the calling thread, which must be the process's main thread.
///
/// # Errors
/// Whatever the platform said when the window could not be created — no display, no GPU, no
/// compositor. The caller should fall back to the console path rather than exit.
pub fn run(controller: UiController, options: GuiOptions) -> Result<(), String> {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([820.0, 560.0])
            .with_title(options.title()),
        ..eframe::NativeOptions::default()
    };
    let app = QuickVibApp::new(controller, options);

    // Not every way a window can fail to open comes back as an `Err`: the X11 keyboard
    // bindings, for one, panic when their shared library is missing. An instrument that is
    // already serving the UTS must not be taken down by that, so the event loop is contained
    // the same way the recording thread is (`docs/PLAN.md` D27) and a panic becomes the same
    // "carry on without a window" path as a returned error.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eframe::run_native(
            "quickvib",
            native,
            Box::new(|_cc| Ok(Box::new(app) as Box<dyn eframe::App>)),
        )
    }));
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("could not open the QuickVib window: {error}")),
        Err(_) => Err("the QuickVib window could not be created on this display".to_owned()),
    }
}

/// Which pane of the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Setup,
    Advanced,
}

/// What the path prompt is going to do with the path it collects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prompt {
    Open,
    SaveAs,
}

struct QuickVibApp {
    controller: UiController,
    options: GuiOptions,
    tab: Tab,
    notice: Option<Notice>,
    field_errors: Vec<crate::FieldError>,
    prompt: Option<Prompt>,
    prompt_path: String,
    export_name: String,
}

impl QuickVibApp {
    fn new(controller: UiController, options: GuiOptions) -> Self {
        let prompt_path = controller
            .path()
            .map_or_else(String::new, |path| path.display().to_string());
        Self {
            controller,
            options,
            tab: Tab::Setup,
            notice: None,
            field_errors: Vec::new(),
            prompt: None,
            prompt_path,
            export_name: "capture.csv".to_owned(),
        }
    }

    fn apply(&mut self) {
        match self.controller.apply() {
            Ok(outcome) => {
                self.field_errors.clear();
                let mut text = "applied to the running instrument".to_owned();
                if let Some(note) = outcome.restart_note() {
                    text.push_str(" - ");
                    text.push_str(&note);
                }
                self.notice = Some(Notice::ok(text));
            }
            Err(errors) => {
                self.notice = Some(Notice::failed(format!(
                    "{} field(s) rejected",
                    errors.len()
                )));
                self.field_errors = errors;
            }
        }
    }

    fn save(&mut self) {
        match self.controller.save() {
            Ok(path) => {
                self.field_errors.clear();
                self.notice = Some(Notice::ok(format!("saved {}", path.display())));
            }
            Err(error) => self.notice = Some(Notice::failed(error)),
        }
    }

    fn finish_prompt(&mut self, prompt: Prompt) {
        let path = self.prompt_path.trim().to_owned();
        if path.is_empty() {
            self.notice = Some(Notice::failed("enter a project path"));
            return;
        }
        let result = match prompt {
            Prompt::Open => self
                .controller
                .load(&path)
                .map(|()| format!("loaded {path}")),
            Prompt::SaveAs => self
                .controller
                .save_as(&path)
                .map(|written| format!("saved {}", written.display())),
        };
        match result {
            Ok(text) => {
                self.field_errors.clear();
                self.notice = Some(Notice::ok(text));
                self.prompt = None;
            }
            Err(error) => self.notice = Some(Notice::failed(error)),
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open project...").clicked() {
                    self.prompt = Some(Prompt::Open);
                    ui.close_menu();
                }
                if ui.button("Save").clicked() {
                    self.save();
                    ui.close_menu();
                }
                if ui.button("Save as...").clicked() {
                    self.prompt = Some(Prompt::SaveAs);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.separator();
            ui.selectable_value(&mut self.tab, Tab::Setup, "Setup");
            ui.selectable_value(&mut self.tab, Tab::Advanced, "Advanced");
            ui.separator();
            let path = self.controller.path().map_or_else(
                || "(unsaved project)".to_owned(),
                |p| p.display().to_string(),
            );
            ui.label(path);
            if self.controller.is_dirty() {
                ui.colored_label(egui::Color32::from_rgb(220, 160, 40), "modified");
            }
        });
    }

    fn setup_tab(&mut self, ui: &mut egui::Ui, running: bool) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_enabled_ui(!running, |ui| {
                self.identity_section(ui);
                ui.separator();
                self.acquisition_section(ui);
                ui.separator();
                self.filter_section(ui);
                ui.separator();
                self.range_section(ui);
                ui.separator();
                self.server_section(ui);
                ui.separator();
                self.export_section(ui);
            });
            if running {
                ui.label("A recording is in flight - stop it to edit the project.");
            }
            ui.separator();
            self.error_list(ui);
        });
    }

    fn identity_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Project");
        egui::Grid::new("project").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.controller.form_mut().name);
            ui.end_row();
            ui.label("Description");
            ui.text_edit_singleline(&mut self.controller.form_mut().description);
            ui.end_row();
        });
    }

    fn acquisition_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Acquisition");
        egui::Grid::new("acquisition")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Sample rate (Hz)");
                let mut rate = self.controller.form().sample_rate_text().to_owned();
                if ui.text_edit_singleline(&mut rate).changed() {
                    self.controller.form_mut().set_sample_rate_text(rate);
                }
                ui.end_row();

                ui.label("Data type");
                let form = self.controller.form_mut();
                egui::ComboBox::from_id_salt("unit")
                    .selected_text(unit_label(form.unit))
                    .show_ui(ui, |ui| {
                        for unit in SampleUnit::all() {
                            ui.selectable_value(&mut form.unit, *unit, unit_label(*unit));
                        }
                    });
                ui.end_row();

                ui.label("Record duration (s)");
                ui.text_edit_singleline(&mut self.controller.form_mut().duration_seconds);
                ui.end_row();

                ui.label("Backend");
                let form = self.controller.form_mut();
                egui::ComboBox::from_id_salt("backend")
                    .selected_text(form.backend.to_string())
                    .show_ui(ui, |ui| {
                        for backend in [BackendKind::Mock, BackendKind::Tcp, BackendKind::M300] {
                            ui.selectable_value(&mut form.backend, backend, backend.to_string());
                        }
                    });
                ui.end_row();
            });
    }

    fn filter_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Filters");
        egui::Grid::new("filters").num_columns(3).show(ui, |ui| {
            ui.label("Low-pass (Hz)");
            let locked = self.controller.form().lpf_locked();
            let mut lpf = self.controller.form().lpf_hz_text().to_owned();
            let response = ui.add_enabled(locked, egui::TextEdit::singleline(&mut lpf));
            if response.changed() {
                self.controller.form_mut().set_lpf_hz_text(lpf);
            }
            let mut pinned = locked;
            if ui.checkbox(&mut pinned, "pin").changed() {
                self.controller.form_mut().set_lpf_locked(pinned);
            }
            ui.end_row();

            ui.label("High-pass (Hz)");
            ui.text_edit_singleline(&mut self.controller.form_mut().high_pass_hz);
            ui.label("0 disables");
            ui.end_row();
        });
        ui.small("Unpinned, the low-pass cutoff follows the Nyquist frequency of the sample rate.");
    }

    fn range_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Measuring ranges");
        egui::Grid::new("ranges").num_columns(2).show(ui, |ui| {
            let active = self.controller.form().unit;
            for (unit, label) in [
                (SampleUnit::VelocityUmPerSec, "Velocity (um/s)"),
                (SampleUnit::DisplacementUm, "Displacement (um)"),
                (SampleUnit::AccelerationMPerSec2, "Acceleration (m/s^2)"),
            ] {
                if unit == active {
                    ui.strong(label);
                } else {
                    ui.label(label);
                }
                let form = self.controller.form_mut();
                let field = match unit {
                    SampleUnit::VelocityUmPerSec => &mut form.velocity_range,
                    SampleUnit::DisplacementUm => &mut form.displacement_range,
                    SampleUnit::AccelerationMPerSec2 => &mut form.acceleration_range,
                };
                ui.text_edit_singleline(field);
                ui.end_row();
            }
        });
    }

    fn server_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ports");
        egui::Grid::new("ports").num_columns(2).show(ui, |ui| {
            ui.label("SCPI port (UTS connects in)");
            ui.text_edit_singleline(&mut self.controller.form_mut().scpi_port);
            ui.end_row();
            ui.label("Device port (M300 dials in)");
            ui.text_edit_singleline(&mut self.controller.form_mut().device_port);
            ui.end_row();
        });
        ui.small("Port changes are saved to the project and take effect at the next start.");
    }

    fn export_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Export");
        egui::Grid::new("export").num_columns(2).show(ui, |ui| {
            ui.label("Format");
            let form = self.controller.form_mut();
            ui.horizontal(|ui| {
                ui.selectable_value(&mut form.export_format, ExportFormat::Csv, "CSV");
                ui.selectable_value(&mut form.export_format, ExportFormat::Txt, "TXT");
            });
            ui.end_row();

            ui.label("Directory");
            ui.text_edit_singleline(&mut self.controller.form_mut().export_directory);
            ui.end_row();

            ui.label("Options");
            let form = self.controller.form_mut();
            ui.vertical(|ui| {
                ui.checkbox(&mut form.include_header, "CSV preamble and header row");
                ui.checkbox(&mut form.remove_dc, "Remove DC before peak and RMS");
            });
            ui.end_row();
        });
    }

    fn error_list(&mut self, ui: &mut egui::Ui) {
        if self.field_errors.is_empty() {
            return;
        }
        ui.colored_label(egui::Color32::from_rgb(210, 80, 80), "Rejected fields");
        for error in &self.field_errors {
            ui.colored_label(
                egui::Color32::from_rgb(210, 80, 80),
                format!("{}: {}", error.field, error.message),
            );
        }
    }

    fn advanced_tab(ui: &mut egui::Ui) {
        ui.heading("Advanced");
        ui.label(
            "Laser power, TEC set point, PID loop gains and external triggering are device \
             controls with no representation in the version-1 project schema, and QuickVib \
             does not invent settings the instrument cannot honour.",
        );
        ui.label(
            "They land here once the schema and the M300 backend carry them; until then the \
             Setup tab is the whole configuration surface.",
        );
    }

    fn status_panel(&mut self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        ui.heading("Instrument");
        egui::Grid::new("status").num_columns(2).show(ui, |ui| {
            ui.label("State");
            ui.strong(status.state.as_scpi_str());
            ui.end_row();

            ui.label("Device");
            if status.connected {
                ui.colored_label(egui::Color32::from_rgb(70, 170, 90), "connected");
            } else {
                ui.colored_label(egui::Color32::from_rgb(210, 80, 80), "not connected");
            }
            ui.end_row();

            if let Some(peer) = status.device_peer {
                ui.label("Peer");
                ui.label(peer.to_string());
                ui.end_row();
            }

            ui.label("SCPI");
            ui.label(format!(
                "port {} - {} session(s)",
                port_text(self.options.scpi_addr),
                status.sessions
            ));
            ui.end_row();

            ui.label("Backend");
            ui.label(self.options.backend.to_string());
            ui.end_row();

            ui.label("*IDN?");
            ui.label(&status.identity);
            ui.end_row();

            ui.label("Duration");
            ui.label(format!("{:.3} s", status.duration_seconds));
            ui.end_row();

            ui.label("Errors queued");
            ui.label(status.pending_errors.to_string());
            ui.end_row();
        });

        ui.separator();
        ui.heading("Last capture");
        match status.measurements {
            Some(measurements) => {
                egui::Grid::new("measurements")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Samples");
                        ui.label(status.sample_count.to_string());
                        ui.end_row();
                        ui.label("Peak");
                        ui.strong(format!("{:.4}", measurements.peak));
                        ui.end_row();
                        ui.label("RMS");
                        ui.strong(format!("{:.4}", measurements.rms));
                        ui.end_row();
                        ui.label("Peak-to-peak");
                        ui.strong(format!("{:.4}", measurements.peak_to_peak));
                        ui.end_row();
                    });
            }
            None => {
                ui.label("no completed capture");
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!status.is_running(), egui::Button::new("Start record"))
                .clicked()
            {
                self.notice = Some(match self.controller.start() {
                    Ok(()) => Notice::ok("recording started"),
                    Err(error) => Notice::failed(error),
                });
            }
            if ui
                .add_enabled(status.is_running(), egui::Button::new("Stop"))
                .clicked()
            {
                self.controller.stop();
                self.notice = Some(Notice::ok("abort requested"));
            }
        });

        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.export_name);
            if ui
                .add_enabled(
                    status.measurements.is_some(),
                    egui::Button::new("Export capture"),
                )
                .clicked()
            {
                self.notice = Some(match self.controller.export(&self.export_name.clone()) {
                    Ok(path) => Notice::ok(format!("wrote {}", path.display())),
                    Err(error) => Notice::failed(error),
                });
            }
        });
    }

    fn path_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompt else {
            return;
        };
        let (title, action) = match prompt {
            Prompt::Open => ("Open project", "Open"),
            Prompt::SaveAs => ("Save project as", "Save"),
        };
        let mut open = true;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Path to a .proj file");
                let response =
                    ui.add(egui::TextEdit::singleline(&mut self.prompt_path).desired_width(420.0));
                let entered =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button(action).clicked() || entered {
                        self.finish_prompt(prompt);
                    }
                    if ui.button("Cancel").clicked() {
                        self.prompt = None;
                    }
                });
            });
        if !open {
            self.prompt = None;
        }
    }
}

impl eframe::App for QuickVibApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let status = self.controller.snapshot();

        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));

        egui::TopBottomPanel::bottom("activity").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(status.headline());
                ui.separator();
                match &self.notice {
                    Some(notice) if notice.ok => {
                        ui.colored_label(egui::Color32::from_rgb(70, 170, 90), &notice.text);
                    }
                    Some(notice) => {
                        ui.colored_label(egui::Color32::from_rgb(210, 80, 80), &notice.text);
                    }
                    None => {
                        ui.label("ready");
                    }
                }
            });
        });

        egui::SidePanel::right("status")
            .default_width(340.0)
            .show(ctx, |ui| self.status_panel(ui, &status));

        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!status.is_running(), egui::Button::new("Apply"))
                    .clicked()
                {
                    self.apply();
                }
                if ui.button("Revert").clicked() {
                    self.controller.revert();
                    self.field_errors.clear();
                    self.notice = Some(Notice::ok("reverted to the loaded project"));
                }
                if ui.button("Save").clicked() {
                    self.save();
                }
                if ui.button("Save as...").clicked() {
                    self.prompt = Some(Prompt::SaveAs);
                }
                if ui.button("Open...").clicked() {
                    self.prompt = Some(Prompt::Open);
                }
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Setup => self.setup_tab(ui, status.is_running()),
            Tab::Advanced => Self::advanced_tab(ui),
        });

        self.path_prompt(ctx);

        // A capture that finishes while nobody is moving the mouse still has to light up the
        // measurement panel, so the window polls the engine a few times a second.
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn unit_label(unit: SampleUnit) -> String {
    format!("{} ({})", unit.as_schema_str(), unit.label())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_title_names_both_ports_and_the_backend() {
        let options = GuiOptions {
            scpi_addr: Some("127.0.0.1:5025".parse().unwrap()),
            device_addr: Some("127.0.0.1:9123".parse().unwrap()),
            backend: BackendKind::Mock,
            version: "1.0.0".to_owned(),
        };
        assert_eq!(
            options.title(),
            "QuickVib 1.0.0 - SCPI 5025 - device 9123 - backend mock"
        );
    }

    #[test]
    fn an_unbound_listener_shows_a_dash() {
        assert_eq!(port_text(None), "-");
    }

    #[test]
    fn unit_labels_carry_both_spellings() {
        assert_eq!(
            unit_label(SampleUnit::VelocityUmPerSec),
            "velocity_um_s (um/s)"
        );
    }
}
