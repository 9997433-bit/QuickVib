//! The egui/eframe window: a factory instrument panel, drawn in Simplified Chinese.
//!
//! Compiled only with the `gui` feature. It owns no rules: every control reads and writes the
//! [`ProjectForm`], every action goes through [`UiController`], every string comes from
//! [`crate::i18n`], every colour comes from the [`Palette`] of the theme in force, and
//! everything it displays about the instrument comes from one [`StatusSnapshot`] taken at the
//! top of the frame. That division is what lets the interesting behaviour be tested without a
//! display.
//!
//! The layout is fixed and deliberately unlike a settings dialog:
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────────────┐
//! │ QuickVib · 项目名   [● 已连接] [● 空闲] [SCPI 15025] [浅色│深色] [中文│EN]      │  title bar
//! ├────────────────────────────────────────────────────────────────────────────────┤
//! │ 文件▾  打开 保存 另存为 │ 应用 还原                        配置 │ 高级          │  toolbar
//! ├──────────────────────────────────────────────┬─────────────────────────────────┤
//! │ 项目信息 / 设备与采样 / 滤波器 / 量程         │        仪表面板                  │
//! │ 录制 / 通信端口 / 数据导出   (滚动)           │  状态 · 开始录制 · 停止          │
//! │                                              │  峰值 / 有效值 / 峰峰值          │
//! ├──────────────────────────────────────────────┴─────────────────────────────────┤
//! │ 空闲 · 已连接        已保存 …            实际监听端口 …          版本 1.0.0     │  status bar
//! └────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The window runs on the main thread — a hard requirement of every desktop windowing system —
//! while the SCPI and device accept loops run on threads behind it, so the instrument keeps
//! answering the UTS while an operator is clicking around.

use std::time::Duration;

use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use quickvib_core::{BackendKind, ExportFormat, SampleUnit};
use quickvib_engine::State;

use crate::appearance::{self, Theme, ThemeStore};
use crate::controller::UiController;
use crate::form::ProjectForm;
use crate::i18n::{self, Label, Lang, LangStore};
use crate::options::GuiOptions;
use crate::ports::PortRow;
use crate::status::{Notice, StatusSnapshot};
use crate::theme::{self, Palette};

/// Open the window and run the event loop until the operator closes it.
///
/// Blocks the calling thread, which must be the process's main thread.
///
/// # Errors
/// Whatever the platform said when the window could not be created — no display, no GPU, no
/// compositor. The caller should fall back to the console path rather than exit.
pub fn run(controller: UiController, options: GuiOptions) -> Result<(), String> {
    let app = QuickVibApp::new(controller, options);
    let theme = app.theme;
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([940.0, 600.0])
            .with_title(app.options.title(app.lang)),
        ..eframe::NativeOptions::default()
    };

    // Not every way a window can fail to open comes back as an `Err`: the X11 keyboard
    // bindings, for one, panic when their shared library is missing. An instrument that is
    // already serving the UTS must not be taken down by that, so the event loop is contained
    // the same way the recording thread is (`docs/PLAN.md` D27) and a panic becomes the same
    // "carry on without a window" path as a returned error.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eframe::run_native(
            "quickvib",
            native,
            Box::new(move |cc| {
                theme::install(&cc.egui_ctx, theme);
                Ok(Box::new(app) as Box<dyn eframe::App>)
            }),
        )
    }));
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("could not open the QuickVib window: {error}")),
        Err(_) => Err("the QuickVib window could not be created on this display".to_owned()),
    }
}

/// Which pane of the configuration side is showing.
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
    lang: Lang,
    lang_store: Option<LangStore>,
    theme: Theme,
    theme_store: Option<ThemeStore>,
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
        let lang_store = LangStore::discover();
        let theme_store = ThemeStore::discover();
        Self {
            controller,
            options,
            lang: i18n::startup_lang(lang_store.as_ref()),
            lang_store,
            theme: appearance::startup_theme(theme_store.as_ref()),
            theme_store,
            tab: Tab::Setup,
            notice: None,
            field_errors: Vec::new(),
            prompt: None,
            prompt_path,
            export_name: "capture.csv".to_owned(),
        }
    }

    /// A fixed string in the language the window is currently drawn in.
    fn t(&self, label: Label) -> &'static str {
        self.lang.t(label)
    }

    /// The colours the window is currently drawn in.
    fn p(&self) -> &'static Palette {
        Palette::of(self.theme)
    }

    // ── Actions ────────────────────────────────────────────────────────────────────────

    fn apply(&mut self) {
        match self.controller.apply() {
            Ok(outcome) => {
                self.field_errors.clear();
                let mut text = self.t(Label::NoticeApplied).to_owned();
                if let Some(note) = outcome.restart_note(self.lang) {
                    text.push_str(" · ");
                    text.push_str(&note);
                }
                self.notice = Some(Notice::ok(text));
            }
            Err(errors) => {
                self.notice = Some(Notice::failed(i18n::rejected_fields(
                    self.lang,
                    errors.len(),
                )));
                self.field_errors = errors;
            }
        }
    }

    fn save(&mut self) {
        match self.controller.save() {
            Ok(path) => {
                self.field_errors.clear();
                self.notice = Some(Notice::ok(i18n::saved(self.lang, &path)));
            }
            Err(error) => self.notice = Some(Notice::failed(error.localized(self.lang))),
        }
    }

    fn finish_prompt(&mut self, prompt: Prompt) {
        let path = self.prompt_path.trim().to_owned();
        if path.is_empty() {
            self.notice = Some(Notice::failed_label(Label::NoticeEmptyPath));
            return;
        }
        let lang = self.lang;
        let result = match prompt {
            Prompt::Open => self
                .controller
                .load(&path)
                .map(|()| i18n::loaded(lang, &path)),
            Prompt::SaveAs => self
                .controller
                .save_as(&path)
                .map(|written| i18n::saved(lang, &written)),
        };
        match result {
            Ok(text) => {
                self.field_errors.clear();
                self.notice = Some(Notice::ok(text));
                self.prompt = None;
            }
            Err(error) => self.notice = Some(Notice::failed(error.localized(lang))),
        }
    }

    fn set_lang(&mut self, ctx: &egui::Context, lang: Lang) {
        if self.lang == lang {
            return;
        }
        self.lang = lang;
        // A window whose chrome is in one language and whose title bar is in another looks
        // broken, and the title is what a screenshot of the instrument shows.
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.options.title(lang)));
        if let Some(store) = &self.lang_store {
            store.write(lang);
        }
    }

    fn set_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        theme::apply(ctx, theme);
        if let Some(store) = &self.theme_store {
            store.write(theme);
        }
    }

    // ── Title bar ──────────────────────────────────────────────────────────────────────

    fn title_bar(&mut self, ctx: &egui::Context, status: &StatusSnapshot) {
        let p = self.p();
        egui::TopBottomPanel::top("title-bar")
            .frame(
                egui::Frame::none()
                    .fill(p.chrome)
                    .inner_margin(egui::Margin::symmetric(14.0, 8.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(
                            RichText::new("QuickVib")
                                .size(20.0)
                                .strong()
                                .color(p.accent),
                        );
                        ui.label(
                            RichText::new(self.t(Label::Tagline))
                                .size(11.0)
                                .color(p.muted),
                        );
                    });
                    ui.add_space(10.0);
                    ui.separator();
                    self.project_identity(ui, status);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.language_switch(ui);
                        ui.add_space(4.0);
                        self.theme_switch(ui);
                        ui.add_space(6.0);
                        theme::chip(
                            ui,
                            p,
                            p.accent,
                            &format!(
                                "{} {} · {} {}",
                                self.t(Label::ScpiShort),
                                self.options.live.scpi_text(),
                                self.t(Label::DeviceShort),
                                self.options.live.device_text()
                            ),
                        );
                        theme::chip(
                            ui,
                            p,
                            state_color(p, status.state),
                            &format!(
                                "{} {}",
                                self.t(Label::RecordState),
                                i18n::state_label(self.lang, status.state)
                            ),
                        );
                        theme::chip(
                            ui,
                            p,
                            if status.connected { p.ok } else { p.bad },
                            self.lang.t(status.link_label()),
                        );
                    });
                });
            });
    }

    fn project_identity(&self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        let p = self.p();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let name = if status.project_name.is_empty() {
                self.t(Label::LabelUnsavedProject).to_owned()
            } else {
                status.project_name.clone()
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).size(15.0).strong());
                if self.controller.is_dirty() {
                    ui.label(
                        RichText::new(self.t(Label::LabelModified))
                            .size(11.0)
                            .color(p.warn),
                    );
                }
            });
            let path = self.controller.path().map_or_else(
                || self.t(Label::LabelUnsavedProject).to_owned(),
                |path| path.display().to_string(),
            );
            ui.label(RichText::new(path).size(11.0).color(p.muted));
        });
    }

    fn language_switch(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let picked = theme::segmented(
            ui,
            self.p(),
            "language-switch",
            76.0,
            self.lang,
            &[
                (Lang::Zh, Lang::Zh.endonym()),
                (Lang::En, Lang::En.endonym()),
            ],
        );
        if let Some(lang) = picked {
            self.set_lang(&ctx, lang);
        }
    }

    /// 浅色 / 深色 beside the language switch: the two things about the window an operator
    /// changes rather than configures, kept together in the one place both are remembered.
    fn theme_switch(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let lang = self.lang;
        let options = Theme::ALL.map(|theme| (theme, theme.name(lang)));
        let picked = theme::segmented(ui, self.p(), "theme-switch", 104.0, self.theme, &options);
        if let Some(theme) = picked {
            self.set_theme(&ctx, theme);
        }
    }

    // ── Toolbar ────────────────────────────────────────────────────────────────────────

    fn toolbar(&mut self, ctx: &egui::Context, running: bool) {
        let p = self.p();
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::none()
                    .fill(p.chrome)
                    .inner_margin(egui::Margin::symmetric(12.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    self.file_menu(ui);
                    ui.separator();
                    if ui.button(self.t(Label::ActionOpen)).clicked() {
                        self.prompt = Some(Prompt::Open);
                    }
                    if ui.button(self.t(Label::ActionSave)).clicked() {
                        self.save();
                    }
                    if ui.button(self.t(Label::ActionSaveAs)).clicked() {
                        self.prompt = Some(Prompt::SaveAs);
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!running, egui::Button::new(self.t(Label::ActionApply)))
                        .clicked()
                    {
                        self.apply();
                    }
                    if ui.button(self.t(Label::ActionRevert)).clicked() {
                        self.controller.revert();
                        self.field_errors.clear();
                        self.notice = Some(Notice::ok_label(Label::NoticeReverted));
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for (tab, label) in [
                            (Tab::Advanced, Label::TabAdvanced),
                            (Tab::Setup, Label::TabSetup),
                        ] {
                            let selected = self.tab == tab;
                            let text = RichText::new(self.lang.t(label)).color(if selected {
                                p.text
                            } else {
                                p.muted
                            });
                            if ui.selectable_label(selected, text).clicked() {
                                self.tab = tab;
                            }
                        }
                    });
                });
            });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(format!("{} ▼", self.t(Label::MenuFile)), |ui| {
            if ui.button(self.t(Label::ActionOpen)).clicked() {
                self.prompt = Some(Prompt::Open);
                ui.close_menu();
            }
            if ui.button(self.t(Label::ActionSave)).clicked() {
                self.save();
                ui.close_menu();
            }
            if ui.button(self.t(Label::ActionSaveAs)).clicked() {
                self.prompt = Some(Prompt::SaveAs);
                ui.close_menu();
            }
            ui.separator();
            if ui.button(self.t(Label::ActionQuit)).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    // ── Configuration side ─────────────────────────────────────────────────────────────

    fn setup_tab(&mut self, ui: &mut egui::Ui, running: bool) {
        let p = self.p();
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if running {
                    banner(ui, p.warn, self.t(Label::HintRunningLock));
                }
                ui.add_enabled_ui(!running, |ui| {
                    self.project_card(ui);
                    self.acquisition_card(ui);
                    self.filter_card(ui);
                    self.range_card(ui);
                    self.recording_card(ui);
                    self.ports_card(ui);
                    self.export_card(ui);
                });
                self.error_card(ui);
            });
    }

    fn project_card(&mut self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionProject), |ui| {
            theme::field_grid(ui, "project-grid", |ui| {
                theme::label(ui, p, lang.t(Label::FieldProjectName));
                theme::wide_text_field(ui, &mut self.controller.form_mut().name);
                ui.end_row();

                theme::label(ui, p, lang.t(Label::FieldDescription));
                theme::wide_text_field(ui, &mut self.controller.form_mut().description);
                ui.end_row();
            });
        });
    }

    fn acquisition_card(&mut self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionAcquisition), |ui| {
            theme::field_grid(ui, "acquisition-grid", |ui| {
                theme::label(ui, p, lang.t(Label::FieldSampleRate));
                let mut rate = self.controller.form().sample_rate_text().to_owned();
                if theme::text_field(ui, &mut rate).changed() {
                    self.controller.form_mut().set_sample_rate_text(rate);
                }
                // The field stays in hertz — that is what the project file and the SCPI
                // surface carry — but nobody reads 100000 as a hundred kilohertz at a glance.
                theme::hint(
                    ui,
                    p,
                    &format!(
                        "Hz  =  {}",
                        i18n::frequency(parse_hz(self.controller.form()))
                    ),
                );
                ui.end_row();

                theme::label(ui, p, lang.t(Label::FieldDataType));
                let form = self.controller.form_mut();
                egui::ComboBox::from_id_salt("unit")
                    .width(theme::FIELD_WIDTH)
                    .selected_text(i18n::unit_label(lang, form.unit))
                    .show_ui(ui, |ui| {
                        for unit in SampleUnit::all() {
                            ui.selectable_value(
                                &mut form.unit,
                                *unit,
                                i18n::unit_label(lang, *unit),
                            );
                        }
                    });
                ui.end_row();

                theme::label(ui, p, lang.t(Label::FieldBackend));
                let form = self.controller.form_mut();
                egui::ComboBox::from_id_salt("backend")
                    .width(theme::FIELD_WIDTH)
                    .selected_text(i18n::backend_label(lang, form.backend))
                    .show_ui(ui, |ui| {
                        for backend in [BackendKind::Mock, BackendKind::Tcp, BackendKind::M300] {
                            ui.selectable_value(
                                &mut form.backend,
                                backend,
                                i18n::backend_label(lang, backend),
                            );
                        }
                    });
                ui.end_row();
            });
        });
    }

    fn filter_card(&mut self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionFilters), |ui| {
            theme::field_grid(ui, "filter-grid", |ui| {
                theme::label(ui, p, lang.t(Label::FieldLowPass));
                let pinned = self.controller.form().lpf_locked();
                let mut lpf = self.controller.form().lpf_hz_text().to_owned();
                let response = ui.add_enabled(
                    pinned,
                    egui::TextEdit::singleline(&mut lpf).desired_width(theme::FIELD_WIDTH),
                );
                if response.changed() {
                    self.controller.form_mut().set_lpf_hz_text(lpf);
                }
                let mut pin = pinned;
                if ui
                    .checkbox(&mut pin, lang.t(Label::FieldPinCutoff))
                    .changed()
                {
                    self.controller.form_mut().set_lpf_locked(pin);
                }
                ui.end_row();

                theme::label(ui, p, lang.t(Label::FieldHighPass));
                theme::text_field(ui, &mut self.controller.form_mut().high_pass_hz);
                theme::hint(ui, p, &format!("Hz  ·  {}", lang.t(Label::HintHighPassOff)));
                ui.end_row();
            });
            ui.add_space(4.0);
            banner(ui, p.accent, lang.t(Label::HintFilterPairing));
            theme::hint(ui, p, lang.t(Label::HintNyquist));
        });
    }

    fn range_card(&mut self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionRanges), |ui| {
            theme::field_grid(ui, "range-grid", |ui| {
                let active = self.controller.form().unit;
                for (unit, label) in [
                    (SampleUnit::VelocityUmPerSec, Label::FieldVelocityRange),
                    (SampleUnit::DisplacementUm, Label::FieldDisplacementRange),
                    (
                        SampleUnit::AccelerationMPerSec2,
                        Label::FieldAccelerationRange,
                    ),
                ] {
                    // The range that belongs to the configured data type is the one in force.
                    if unit == active {
                        ui.label(RichText::new(lang.t(label)).strong().color(p.text));
                    } else {
                        theme::label(ui, p, lang.t(label));
                    }
                    let form = self.controller.form_mut();
                    let field = match unit {
                        SampleUnit::VelocityUmPerSec => &mut form.velocity_range,
                        SampleUnit::DisplacementUm => &mut form.displacement_range,
                        SampleUnit::AccelerationMPerSec2 => &mut form.acceleration_range,
                    };
                    theme::text_field(ui, field);
                    theme::hint(ui, p, i18n::unit_symbol(unit));
                    ui.end_row();
                }
            });
        });
    }

    fn recording_card(&mut self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionRecording), |ui| {
            theme::field_grid(ui, "recording-grid", |ui| {
                theme::label(ui, p, lang.t(Label::FieldDuration));
                theme::text_field(ui, &mut self.controller.form_mut().duration_seconds);
                theme::hint(ui, p, i18n::seconds(lang, 1.0).trim_start_matches("1 "));
                ui.end_row();
            });
        });
    }

    fn ports_card(&mut self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        let rows = self.port_rows();
        theme::card(ui, p, lang.t(Label::SectionPorts), |ui| {
            // What is bound right now: fact, not setting. The window never shows the
            // project's copy of a port where the live one belongs.
            ui.label(
                RichText::new(lang.t(Label::ListeningPorts))
                    .size(12.5)
                    .color(p.muted),
            );
            ui.add_space(4.0);
            theme::field_grid(ui, "live-ports-grid", |ui| {
                theme::label(ui, p, lang.t(Label::ScpiShort));
                theme::readout(ui, &self.options.live.scpi_text(), p.ok);
                theme::hint(ui, p, lang.t(Label::HintScpiPeer));
                ui.end_row();

                theme::label(ui, p, lang.t(Label::DeviceShort));
                theme::readout(ui, &self.options.live.device_text(), p.ok);
                theme::hint(ui, p, lang.t(Label::HintDevicePeer));
                ui.end_row();
            });
            theme::hint(ui, p, lang.t(Label::HintLivePorts));

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            ui.label(
                RichText::new(lang.t(Label::FieldProjectPorts))
                    .size(12.5)
                    .color(p.muted),
            );
            ui.add_space(4.0);
            theme::field_grid(ui, "project-ports-grid", |ui| {
                theme::label(ui, p, lang.t(Label::FieldScpiPort));
                theme::text_field(ui, &mut self.controller.form_mut().scpi_port);
                ui.end_row();

                theme::label(ui, p, lang.t(Label::FieldDevicePort));
                theme::text_field(ui, &mut self.controller.form_mut().device_port);
                ui.end_row();
            });
            theme::hint(ui, p, lang.t(Label::HintProjectPorts));
            if crate::ports::any_mismatch(&rows) {
                ui.add_space(6.0);
                banner(ui, p.warn, lang.t(Label::HintPortMismatch));
            }
        });
    }

    fn export_card(&mut self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionExport), |ui| {
            theme::field_grid(ui, "export-grid", |ui| {
                theme::label(ui, p, lang.t(Label::FieldExportFormat));
                let form = self.controller.form_mut();
                ui.horizontal(|ui| {
                    for format in [ExportFormat::Csv, ExportFormat::Txt] {
                        ui.selectable_value(
                            &mut form.export_format,
                            format,
                            i18n::format_label(format),
                        );
                    }
                });
                ui.end_row();

                theme::label(ui, p, lang.t(Label::FieldExportDirectory));
                theme::wide_text_field(ui, &mut self.controller.form_mut().export_directory);
                theme::hint(ui, p, lang.t(Label::HintExportDirectory));
                ui.end_row();

                theme::label(ui, p, lang.t(Label::FieldExportOptions));
                let form = self.controller.form_mut();
                ui.vertical(|ui| {
                    ui.checkbox(&mut form.include_header, lang.t(Label::OptionCsvHeader));
                    ui.checkbox(&mut form.remove_dc, lang.t(Label::OptionRemoveDc));
                });
                ui.end_row();
            });
        });
    }

    fn error_card(&mut self, ui: &mut egui::Ui) {
        if self.field_errors.is_empty() {
            return;
        }
        let (lang, p) = (self.lang, self.p());
        egui::Frame::none()
            .fill(p.bad.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, p.bad))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(14.0, 12.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(lang.t(Label::ErrorsHeading))
                        .strong()
                        .color(p.bad),
                );
                ui.add_space(4.0);
                for error in &self.field_errors {
                    ui.label(RichText::new(error.localized(lang)).color(p.bad));
                }
            });
        ui.add_space(10.0);
    }

    fn advanced_tab(&self, ui: &mut egui::Ui) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::AdvancedTitle), |ui| {
            ui.label(RichText::new(lang.t(Label::AdvancedBody)).color(p.muted));
            ui.add_space(8.0);
            ui.label(RichText::new(lang.t(Label::AdvancedBody2)).color(p.muted));
        });
    }

    // ── Instrument column ──────────────────────────────────────────────────────────────

    fn instrument_panel(&mut self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        let lang = self.lang;
        ui.add_space(10.0);
        ui.label(
            RichText::new(lang.t(Label::SectionInstrument))
                .size(16.0)
                .strong(),
        );
        ui.add_space(10.0);

        self.state_block(ui, status);
        ui.add_space(10.0);
        self.transport_buttons(ui, status);
        ui.add_space(12.0);
        self.measurement_block(ui, status);
        ui.add_space(10.0);
        self.export_row(ui, status);
        ui.add_space(12.0);
        self.link_block(ui, status);
    }

    fn state_block(&self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        let (lang, p) = (self.lang, self.p());
        let color = state_color(p, status.state);
        egui::Frame::none()
            .fill(color.gamma_multiply(0.14))
            .stroke(egui::Stroke::new(1.0, color))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(14.0, 12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(lang.t(Label::RecordState))
                        .size(12.0)
                        .color(p.muted),
                );
                ui.horizontal(|ui| {
                    theme::led(ui, color);
                    ui.label(
                        RichText::new(i18n::state_label(lang, status.state))
                            .size(26.0)
                            .strong()
                            .color(color),
                    );
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(lang.t(Label::LinkState))
                            .size(12.0)
                            .color(p.muted),
                    );
                    theme::led(ui, if status.connected { p.ok } else { p.bad });
                    ui.label(RichText::new(lang.t(status.link_label())).size(13.0));
                });
            });
    }

    fn transport_buttons(&mut self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        let (lang, p) = (self.lang, self.p());
        let running = status.is_running();
        let width = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            if theme::action_button(ui, p, lang.t(Label::ActionStart), p.ok, !running, width)
                .clicked()
            {
                self.notice = Some(match self.controller.start() {
                    Ok(()) => Notice::ok_label(Label::NoticeStarted),
                    Err(error) => Notice::failed(error.localized(lang)),
                });
            }
            if theme::action_button(ui, p, lang.t(Label::ActionStop), p.live, running, width)
                .clicked()
            {
                self.controller.stop();
                self.notice = Some(Notice::ok_label(Label::NoticeStopRequested));
            }
        });
    }

    fn measurement_block(&self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionMeasurements), |ui| {
            ui.set_width(ui.available_width());
            match status.measurements {
                Some(measurements) => {
                    for (label, value) in [
                        (Label::MeasPeak, measurements.peak),
                        (Label::MeasRms, measurements.rms),
                        (Label::MeasPeakToPeak, measurements.peak_to_peak),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(lang.t(label)).size(13.0).color(p.muted));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(i18n::measurement(value, status.unit))
                                        .monospace()
                                        .size(17.0)
                                        .strong()
                                        .color(p.text),
                                );
                            });
                        });
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);
                    small_row(
                        ui,
                        p,
                        lang.t(Label::MeasSamples),
                        &status.sample_count.to_string(),
                    );
                    small_row(
                        ui,
                        p,
                        lang.t(Label::LabelDuration),
                        &i18n::seconds(lang, status.duration_seconds),
                    );
                }
                None => {
                    ui.label(
                        RichText::new(lang.t(Label::NoCapture))
                            .size(13.0)
                            .color(p.muted),
                    );
                }
            }
        });
    }

    fn export_row(&mut self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            // The button is sized by its own label — which is longer in English — and the
            // file name takes whatever is left, so neither is ever clipped.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        status.measurements.is_some(),
                        egui::Button::new(lang.t(Label::ActionExport)),
                    )
                    .clicked()
                {
                    let name = self.export_name.clone();
                    self.notice = Some(match self.controller.export(&name) {
                        Ok(path) => Notice::ok(i18n::wrote(lang, &path)),
                        Err(error) => Notice::failed(error.localized(lang)),
                    });
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.export_name)
                        .desired_width(ui.available_width())
                        .hint_text(lang.t(Label::FieldFileName)),
                );
            });
        });
    }

    fn link_block(&self, ui: &mut egui::Ui, status: &StatusSnapshot) {
        let (lang, p) = (self.lang, self.p());
        theme::card(ui, p, lang.t(Label::SectionLink), |ui| {
            ui.set_width(ui.available_width());
            small_row(
                ui,
                p,
                lang.t(Label::LabelBackend),
                i18n::backend_label(lang, self.options.backend),
            );
            small_row(
                ui,
                p,
                lang.t(Label::FieldSampleRate),
                &i18n::frequency(status.sample_rate_hz),
            );
            small_row(
                ui,
                p,
                lang.t(Label::LabelPeer),
                &status.device_peer.map_or_else(
                    || lang.t(Label::LabelNone).to_owned(),
                    |peer| peer.to_string(),
                ),
            );
            small_row(
                ui,
                p,
                lang.t(Label::LabelSessions),
                &status.sessions.to_string(),
            );
            small_row(
                ui,
                p,
                lang.t(Label::LabelPendingErrors),
                &status.pending_errors.to_string(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(lang.t(Label::LabelIdentity))
                    .size(12.0)
                    .color(p.muted),
            );
            ui.label(RichText::new(&status.identity).size(11.5).monospace());
        });
    }

    // ── Status bar and dialogs ─────────────────────────────────────────────────────────

    fn status_bar(&self, ctx: &egui::Context, status: &StatusSnapshot) {
        let (lang, p) = (self.lang, self.p());
        egui::TopBottomPanel::bottom("status-bar")
            .exact_height(30.0)
            .frame(
                egui::Frame::none()
                    .fill(p.chrome)
                    .inner_margin(egui::Margin::symmetric(14.0, 5.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    theme::led(ui, state_color(p, status.state));
                    ui.label(RichText::new(status.headline(lang)).size(12.5));
                    ui.separator();
                    match &self.notice {
                        Some(notice) if notice.ok => {
                            ui.label(RichText::new(notice.text(lang)).size(12.5).color(p.ok));
                        }
                        Some(notice) => {
                            ui.label(RichText::new(notice.text(lang)).size(12.5).color(p.bad));
                        }
                        None => {
                            ui.label(
                                RichText::new(lang.t(Label::NoticeReady))
                                    .size(12.5)
                                    .color(p.muted),
                            );
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} {}",
                                lang.t(Label::LabelVersion),
                                self.options.version
                            ))
                            .size(12.0)
                            .color(p.muted),
                        );
                        ui.separator();
                        // The ports here are the bound ones, always — the status bar is the
                        // one place an operator should never have to wonder about that.
                        ui.label(
                            RichText::new(format!(
                                "{}: {} {} · {} {}",
                                lang.t(Label::ListeningPorts),
                                lang.t(Label::ScpiShort),
                                self.options.live.scpi_text(),
                                lang.t(Label::DeviceShort),
                                self.options.live.device_text()
                            ))
                            .size(12.0)
                            .color(p.muted),
                        );
                    });
                });
            });
    }

    fn path_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompt else {
            return;
        };
        let p = self.p();
        let (title, action) = match prompt {
            Prompt::Open => (Label::DialogOpen, Label::ActionOpen),
            Prompt::SaveAs => (Label::DialogSaveAs, Label::ActionSave),
        };
        let mut open = true;
        egui::Window::new(self.t(title))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(self.t(Label::DialogPath))
                        .size(12.5)
                        .color(p.muted),
                );
                ui.add_space(4.0);
                let response =
                    ui.add(egui::TextEdit::singleline(&mut self.prompt_path).desired_width(440.0));
                let entered =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(self.t(action)).clicked() || entered {
                        self.finish_prompt(prompt);
                    }
                    if ui.button(self.t(Label::ActionCancel)).clicked() {
                        self.prompt = None;
                    }
                });
            });
        if !open {
            self.prompt = None;
        }
    }

    /// The bound ports against the ports the form is editing, for the mismatch warning.
    fn port_rows(&self) -> [PortRow; 2] {
        let form = self.controller.form();
        [
            PortRow::new(self.options.live.scpi_port(), &form.scpi_port),
            PortRow::new(self.options.live.device_port(), &form.device_port),
        ]
    }
}

impl eframe::App for QuickVibApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let status = self.controller.snapshot();
        let running = status.is_running();
        let p = self.p();

        self.title_bar(ctx, &status);
        self.toolbar(ctx, running);
        self.status_bar(ctx, &status);

        egui::SidePanel::right("instrument")
            .exact_width(theme::INSTRUMENT_WIDTH)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(p.instrument)
                    .inner_margin(egui::Margin::symmetric(14.0, 0.0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| self.instrument_panel(ui, &status));
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(p.backdrop)
                    .inner_margin(egui::Margin::symmetric(14.0, 12.0)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Setup => self.setup_tab(ui, running),
                Tab::Advanced => self.advanced_tab(ui),
            });

        self.path_prompt(ctx);

        // A capture that finishes while nobody is moving the mouse still has to light up the
        // measurement panel, so the window polls the engine a few times a second.
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

/// The colour that stands for an instrument state, used by the LED, the chip and the big
/// state block so all three always agree.
fn state_color(p: &Palette, state: State) -> Color32 {
    match state {
        State::Idle => p.muted,
        State::Armed => p.warn,
        State::Recording => p.live,
        State::Complete => p.ok,
        State::Aborted => p.bad,
    }
}

/// A full-width tinted strip: the running lock, the filter pairing rule, the port mismatch.
fn banner(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.13))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.7)))
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 20.0);
            ui.label(RichText::new(text).size(12.5).color(color));
        });
    ui.add_space(8.0);
}

/// A dim label with its value pushed to the right edge, as used all down the instrument
/// column.
fn small_row(ui: &mut egui::Ui, p: &Palette, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0).color(p.muted));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).size(12.5));
        });
    });
}

/// The sample rate currently in the form, for the kilohertz readout beside the field.
fn parse_hz(form: &ProjectForm) -> f64 {
    form.sample_rate_text().trim().parse::<f64>().unwrap_or(0.0)
}
