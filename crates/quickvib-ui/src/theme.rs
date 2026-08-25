//! The look of the instrument: fonts, palette, and the two containers every panel is built
//! from.
//!
//! Compiled only with the `gui` feature. Kept apart from [`crate::window`] so the layout code
//! reads as layout — a card here, a field row there — instead of as a wall of colour
//! constants, and so that "every card has the same padding" is one function rather than a
//! convention nobody can enforce.

use eframe::egui::{
    self, Align, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Layout, Margin,
    Response, RichText, Rounding, Stroke, TextStyle, Ui, Vec2,
};
use std::sync::Arc;

use crate::font::{CJK_FONT, CJK_FONT_NAME};

/// The instrument palette: a dark console with one accent per meaning, so a glance across the
/// room says what state the machine is in.
pub struct Palette;

impl Palette {
    /// The window behind everything.
    pub const BACKDROP: Color32 = Color32::from_rgb(0x12, 0x16, 0x1C);
    /// The title bar and the toolbar.
    pub const CHROME: Color32 = Color32::from_rgb(0x18, 0x1E, 0x27);
    /// A configuration card.
    pub const CARD: Color32 = Color32::from_rgb(0x1D, 0x24, 0x2E);
    /// The instrument column, one step darker than the cards beside it.
    pub const INSTRUMENT: Color32 = Color32::from_rgb(0x16, 0x1C, 0x24);
    /// Card and panel borders.
    pub const EDGE: Color32 = Color32::from_rgb(0x2B, 0x35, 0x43);
    /// Body text.
    pub const TEXT: Color32 = Color32::from_rgb(0xE3, 0xE9, 0xF2);
    /// Secondary text: hints, units, help lines.
    pub const MUTED: Color32 = Color32::from_rgb(0x93, 0xA1, 0xB4);
    /// The product accent, used for headings and the active tab.
    pub const ACCENT: Color32 = Color32::from_rgb(0x39, 0xA8, 0xE0);
    /// Everything is well: connected, applied, complete.
    pub const OK: Color32 = Color32::from_rgb(0x35, 0xB0, 0x6B);
    /// A capture is in flight.
    pub const LIVE: Color32 = Color32::from_rgb(0xE2, 0x54, 0x54);
    /// Something needs attention but nothing is broken.
    pub const WARN: Color32 = Color32::from_rgb(0xE0, 0xA5, 0x30);
    /// A rejection.
    pub const BAD: Color32 = Color32::from_rgb(0xE2, 0x6D, 0x6D);
}

/// Width of the instrument column on the right.
pub const INSTRUMENT_WIDTH: f32 = 372.0;
/// Width of the label column in every configuration card, so labels line up down the whole
/// window rather than per card.
pub const LABEL_WIDTH: f32 = 116.0;
/// Width of a numeric field. One number, so nothing in the form is ragged.
pub const FIELD_WIDTH: f32 = 168.0;
/// Width of a field holding a name or a path, which needs more room than a number.
pub const WIDE_FIELD_WIDTH: f32 = 300.0;

/// Install the font, the palette and the spacing. Called once, when the window is created.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.5)),
        (TextStyle::Heading, FontId::proportional(17.0)),
        (TextStyle::Monospace, FontId::monospace(14.0)),
    ]
    .into();

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.menu_margin = Margin::same(6.0);
    style.spacing.interact_size.y = 26.0;

    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.panel_fill = Palette::BACKDROP;
    visuals.window_fill = Palette::CARD;
    visuals.extreme_bg_color = Color32::from_rgb(0x10, 0x14, 0x1A);
    visuals.faint_bg_color = Color32::from_rgb(0x22, 0x2A, 0x35);
    visuals.override_text_color = Some(Palette::TEXT);
    visuals.window_stroke = Stroke::new(1.0, Palette::EDGE);
    visuals.window_rounding = Rounding::same(8.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals.selection.bg_fill = Palette::ACCENT.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, Palette::TEXT);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Palette::EDGE);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x25, 0x2E, 0x3A);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x25, 0x2E, 0x3A);
    visuals.widgets.inactive.rounding = Rounding::same(5.0);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x2F, 0x3A, 0x49);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2F, 0x3A, 0x49);
    visuals.widgets.hovered.rounding = Rounding::same(5.0);
    visuals.widgets.active.bg_fill = Palette::ACCENT.gamma_multiply(0.5);
    visuals.widgets.active.weak_bg_fill = Palette::ACCENT.gamma_multiply(0.5);
    visuals.widgets.active.rounding = Rounding::same(5.0);
    visuals.widgets.open.rounding = Rounding::same(5.0);

    ctx.set_style(style);
}

/// Put the embedded CJK face in front of the built-in Latin ones.
///
/// Order matters: egui walks the family list per glyph, so the Chinese face answers for
/// hanzi and punctuation while the stock fonts still cover anything it was subset out of.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        CJK_FONT_NAME.to_owned(),
        Arc::new(FontData::from_static(CJK_FONT)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, CJK_FONT_NAME.to_owned());
    }
    ctx.set_fonts(fonts);
}

/// One configuration group: a titled, bordered box with the same padding as every other.
pub fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = Frame::none()
        .fill(Palette::CARD)
        .stroke(Stroke::new(1.0, Palette::EDGE))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            // Cards that shrink to their content leave a ragged right edge down the column,
            // which is exactly the "pile of settings" look the panel is trying not to have.
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                // A short accent rule instead of a bigger font: the eye finds the group
                // without the type size shouting.
                let (rect, _) = ui.allocate_exact_size(Vec2::new(3.0, 15.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, Rounding::same(2.0), Palette::ACCENT);
                ui.add_space(2.0);
                ui.label(RichText::new(title).size(15.0).strong());
            });
            ui.add_space(8.0);
            add(ui)
        });
    ui.add_space(10.0);
    inner.inner
}

/// The field grid every card uses: label, control, unit or hint — aligned across all cards.
pub fn field_grid<R>(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Grid::new(id)
        .num_columns(3)
        .spacing(Vec2::new(12.0, 9.0))
        .min_col_width(LABEL_WIDTH)
        .max_col_width(WIDE_FIELD_WIDTH + 8.0)
        .show(ui, add)
        .inner
}

/// A field label in the left column of a [`field_grid`].
pub fn label(ui: &mut Ui, text: &str) {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new(text).color(Palette::MUTED));
    });
}

/// The dimmed unit or hint in the right column of a [`field_grid`].
pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(Palette::MUTED));
}

/// A single-line editor of the one width every numeric field in the window uses.
pub fn text_field(ui: &mut Ui, value: &mut String) -> Response {
    ui.add(egui::TextEdit::singleline(value).desired_width(FIELD_WIDTH))
}

/// A single-line editor for a name or a path.
pub fn wide_text_field(ui: &mut Ui, value: &mut String) -> Response {
    ui.add(egui::TextEdit::singleline(value).desired_width(WIDE_FIELD_WIDTH))
}

/// A read-only value shown where an editable field would be — a fact, not a setting.
pub fn readout(ui: &mut Ui, text: &str, color: Color32) {
    ui.label(
        RichText::new(text)
            .monospace()
            .size(14.0)
            .strong()
            .color(color),
    );
}

/// A status chip: a coloured dot and a word, as used all along the title bar.
pub fn chip(ui: &mut Ui, dot: Color32, text: &str) {
    Frame::none()
        .fill(Palette::BACKDROP)
        .rounding(Rounding::same(11.0))
        .inner_margin(Margin::symmetric(9.0, 3.0))
        .stroke(Stroke::new(1.0, Palette::EDGE))
        .show(ui, |ui| {
            led(ui, dot);
            ui.label(RichText::new(text).size(12.5));
        });
}

/// The coloured dot itself, on its own.
pub fn led(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 9.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
}

/// A big coloured button of a given width — the two that start and stop a capture.
pub fn action_button(
    ui: &mut Ui,
    text: &str,
    fill: Color32,
    enabled: bool,
    width: f32,
) -> Response {
    let button = egui::Button::new(RichText::new(text).size(15.0).strong().color(if enabled {
        Color32::WHITE
    } else {
        Palette::MUTED
    }))
    .fill(if enabled {
        fill
    } else {
        Color32::from_rgb(0x23, 0x2B, 0x36)
    })
    .rounding(Rounding::same(6.0))
    .min_size(Vec2::new(width, 36.0));
    ui.add_enabled(enabled, button)
}
