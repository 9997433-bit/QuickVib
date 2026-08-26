//! The look of the instrument: fonts, the two palettes, and the containers every panel is
//! built from.
//!
//! Compiled only with the `gui` feature. Kept apart from [`crate::window`] so the layout code
//! reads as layout — a card here, a field row there — instead of as a wall of colour
//! constants, and so that "every card has the same padding" is one function rather than a
//! convention nobody can enforce.
//!
//! There are two palettes and nothing else varies between them: the same layout, the same
//! spacing, the same fonts. Which one is in force is [`crate::appearance::Theme`], which lives
//! outside this module because the choice — and the file it is remembered in — needs no
//! display and is therefore unit-tested in the ordinary `cargo test` run.

use eframe::egui::{
    self, Align, Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Layout, Margin,
    Response, RichText, Rounding, Stroke, TextStyle, Ui, Vec2,
};
use std::sync::Arc;

use crate::appearance::Theme;
use crate::font::{CJK_FONT, CJK_FONT_NAME};

/// One palette: an accent per meaning, so a glance across the room says what state the machine
/// is in, and a set of surfaces that keep the panel legible at either end of the light.
///
/// Both palettes carry every field, so there is no "if light then" anywhere in the layout
/// code — a control asks the palette for the colour of what it means, not for a shade.
pub struct Palette {
    /// The window behind everything.
    pub backdrop: Color32,
    /// The title bar, the toolbar and the status bar.
    pub chrome: Color32,
    /// A configuration card.
    pub card: Color32,
    /// The instrument column, set off from the cards beside it.
    pub instrument: Color32,
    /// Card and panel borders.
    pub edge: Color32,
    /// Body text.
    pub text: Color32,
    /// Emphasised text: card titles, the project name, a bold reading. What egui calls the
    /// strong text colour, and the one colour that must out-contrast [`Palette::text`].
    pub strong: Color32,
    /// Secondary text: hints, units, help lines.
    pub muted: Color32,
    /// The product accent, used for headings and the active tab.
    pub accent: Color32,
    /// Everything is well: connected, applied, complete.
    pub ok: Color32,
    /// A capture is in flight.
    pub live: Color32,
    /// Something needs attention but nothing is broken.
    pub warn: Color32,
    /// A rejection.
    pub bad: Color32,
    /// The inside of a text field — the one surface that reads as "type here".
    pub sunken: Color32,
    /// A faintly separated strip: table stripes, collapsed headers.
    pub faint: Color32,
    /// A button or combo box at rest.
    pub control: Color32,
    /// The same, under the pointer.
    pub control_hover: Color32,
    /// The same, held down.
    pub pressed: Color32,
    /// The outline around a control. A light panel needs one to tell a white field from the
    /// white card under it; the dark console never had one and does not get one now.
    pub control_stroke: Stroke,
    /// The fill behind a selected item and behind selected text.
    pub selection: Color32,
    /// A control that cannot be used right now.
    pub disabled: Color32,
}

/// The dark console: the shipped look, and what a first launch shows.
pub const DARK: Palette = Palette {
    backdrop: Color32::from_rgb(0x12, 0x16, 0x1C),
    chrome: Color32::from_rgb(0x18, 0x1E, 0x27),
    card: Color32::from_rgb(0x1D, 0x24, 0x2E),
    instrument: Color32::from_rgb(0x16, 0x1C, 0x24),
    edge: Color32::from_rgb(0x2B, 0x35, 0x43),
    text: Color32::from_rgb(0xE3, 0xE9, 0xF2),
    strong: Color32::WHITE,
    muted: Color32::from_rgb(0x93, 0xA1, 0xB4),
    accent: Color32::from_rgb(0x39, 0xA8, 0xE0),
    ok: Color32::from_rgb(0x35, 0xB0, 0x6B),
    live: Color32::from_rgb(0xE2, 0x54, 0x54),
    warn: Color32::from_rgb(0xE0, 0xA5, 0x30),
    bad: Color32::from_rgb(0xE2, 0x6D, 0x6D),
    sunken: Color32::from_rgb(0x10, 0x14, 0x1A),
    faint: Color32::from_rgb(0x22, 0x2A, 0x35),
    control: Color32::from_rgb(0x25, 0x2E, 0x3A),
    control_hover: Color32::from_rgb(0x2F, 0x3A, 0x49),
    pressed: Color32::from_rgb(0x2A, 0x63, 0x86),
    control_stroke: Stroke::NONE,
    selection: Color32::from_rgb(0x1E, 0x54, 0x74),
    disabled: Color32::from_rgb(0x23, 0x2B, 0x36),
};

/// The light panel: the same instrument under bench lighting.
///
/// Not the dark palette inverted. The accents are darkened until each one carries at least a
/// 4.5:1 contrast ratio against the card it is drawn on, because 录制中 in the dark theme's
/// red is a pale smudge on white, and a state an operator cannot read is worse than no colour
/// at all.
pub const LIGHT: Palette = Palette {
    backdrop: Color32::from_rgb(0xED, 0xF0, 0xF4),
    chrome: Color32::from_rgb(0xDE, 0xE4, 0xEB),
    card: Color32::from_rgb(0xFA, 0xFB, 0xFD),
    instrument: Color32::from_rgb(0xE5, 0xEA, 0xF0),
    edge: Color32::from_rgb(0xBC, 0xC5, 0xD2),
    text: Color32::from_rgb(0x16, 0x1E, 0x29),
    strong: Color32::from_rgb(0x08, 0x0D, 0x14),
    muted: Color32::from_rgb(0x4D, 0x5A, 0x6D),
    accent: Color32::from_rgb(0x0A, 0x63, 0x99),
    ok: Color32::from_rgb(0x18, 0x6E, 0x3F),
    live: Color32::from_rgb(0xBA, 0x25, 0x25),
    warn: Color32::from_rgb(0x7E, 0x53, 0x05),
    bad: Color32::from_rgb(0xA8, 0x22, 0x22),
    sunken: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    faint: Color32::from_rgb(0xE7, 0xEC, 0xF2),
    control: Color32::from_rgb(0xF4, 0xF7, 0xFB),
    control_hover: Color32::from_rgb(0xE6, 0xEC, 0xF4),
    pressed: Color32::from_rgb(0xC6, 0xDF, 0xF2),
    control_stroke: Stroke {
        width: 1.0,
        color: Color32::from_rgb(0xB5, 0xC0, 0xCF),
    },
    selection: Color32::from_rgb(0xC2, 0xDE, 0xF3),
    disabled: Color32::from_rgb(0xDA, 0xDF, 0xE7),
};

impl Palette {
    /// The palette of `theme`.
    #[must_use]
    pub const fn of(theme: Theme) -> &'static Self {
        match theme {
            Theme::Dark => &DARK,
            Theme::Light => &LIGHT,
        }
    }

    /// The colour text takes when it sits on top of one of the saturated fills — a transport
    /// button, a filled chip. Both palettes keep those fills dark enough for white.
    pub const ON_FILL: Color32 = Color32::WHITE;
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
pub fn install(ctx: &egui::Context, theme: Theme) {
    install_fonts(ctx);
    apply(ctx, theme);
}

/// Repaint the whole window in `theme`.
///
/// Called again every time the operator flips the switch: egui holds no per-widget colour
/// state, so replacing the style is the whole of a theme change and it takes effect on the
/// next frame, with no restart and nothing to reload.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let palette = Palette::of(theme);
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
    // egui reads this flag when it picks a shadow or a default it was not given, so it has to
    // agree with the palette even though the panel names every colour it cares about.
    visuals.dark_mode = theme.is_dark();
    visuals.panel_fill = palette.backdrop;
    visuals.window_fill = palette.card;
    visuals.extreme_bg_color = palette.sunken;
    visuals.faint_bg_color = palette.faint;
    visuals.override_text_color = Some(palette.text);
    visuals.window_stroke = Stroke::new(1.0, palette.edge);
    visuals.window_rounding = Rounding::same(8.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals.selection.bg_fill = palette.selection;
    visuals.selection.stroke = Stroke::new(1.0, palette.text);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.edge);
    // egui fades a disabled widget towards this one colour, so a light panel that leaves it
    // at the dark-mode default grows black buttons wherever a control is out of reach.
    visuals.widgets.noninteractive.weak_bg_fill = palette.backdrop;
    visuals.widgets.inactive.bg_fill = palette.control;
    visuals.widgets.inactive.weak_bg_fill = palette.control;
    visuals.widgets.inactive.bg_stroke = palette.control_stroke;
    visuals.widgets.inactive.rounding = Rounding::same(5.0);
    visuals.widgets.hovered.bg_fill = palette.control_hover;
    visuals.widgets.hovered.weak_bg_fill = palette.control_hover;
    visuals.widgets.hovered.rounding = Rounding::same(5.0);
    visuals.widgets.active.bg_fill = palette.pressed;
    visuals.widgets.active.weak_bg_fill = palette.pressed;
    visuals.widgets.active.rounding = Rounding::same(5.0);
    visuals.widgets.open.rounding = Rounding::same(5.0);

    // Text that names its own colour is unaffected, but the strings that do not — a card
    // title, a checkbox tick, a combo box arrow — read their colour off the widget state
    // egui is drawing. Left at the dark-mode defaults, every one of them is white, which on
    // the light panel is a heading nobody can see.
    for (state, width) in [
        (&mut visuals.widgets.noninteractive, 1.0),
        (&mut visuals.widgets.inactive, 1.0),
        (&mut visuals.widgets.open, 1.0),
    ] {
        state.fg_stroke = Stroke::new(width, palette.text);
    }
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, palette.strong);
    // `Visuals::strong_text_color` is this one, and it is what every `RichText::strong` in
    // the window resolves to.
    visuals.widgets.active.fg_stroke = Stroke::new(2.0, palette.strong);

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
pub fn card<R>(ui: &mut Ui, p: &Palette, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = Frame::none()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.edge))
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
                    .rect_filled(rect, Rounding::same(2.0), p.accent);
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
pub fn label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new(text).color(p.muted));
    });
}

/// The dimmed unit or hint in the right column of a [`field_grid`].
pub fn hint(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(p.muted));
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
pub fn chip(ui: &mut Ui, p: &Palette, dot: Color32, text: &str) {
    Frame::none()
        .fill(p.backdrop)
        .rounding(Rounding::same(11.0))
        .inner_margin(Margin::symmetric(9.0, 3.0))
        .stroke(Stroke::new(1.0, p.edge))
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
    p: &Palette,
    text: &str,
    fill: Color32,
    enabled: bool,
    width: f32,
) -> Response {
    let button = egui::Button::new(RichText::new(text).size(15.0).strong().color(if enabled {
        Palette::ON_FILL
    } else {
        p.muted
    }))
    .fill(if enabled { fill } else { p.disabled })
    .rounding(Rounding::same(6.0))
    .min_size(Vec2::new(width, 36.0));
    ui.add_enabled(enabled, button)
}

/// A segmented switch of the kind the title bar carries: two or more short labels in one
/// bordered strip, of which exactly one is lit. Returns the value the operator picked, if any.
///
/// The title bar lays its contents out from the right, and egui propagates that direction into
/// nested rows — so the strip asks for a region of its own size and its own direction, and
/// reads left to right either way.
pub fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    p: &Palette,
    id: &str,
    width: f32,
    current: T,
    options: &[(T, &str)],
) -> Option<T> {
    let mut picked = None;
    Frame::none()
        .fill(p.backdrop)
        .rounding(Rounding::same(6.0))
        .stroke(Stroke::new(1.0, p.edge))
        .inner_margin(Margin::symmetric(4.0, 3.0))
        .show(ui, |ui| {
            ui.push_id(id, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 20.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        for (value, text) in options {
                            let selected = *value == current;
                            let label = RichText::new(*text).size(13.0).color(if selected {
                                p.text
                            } else {
                                p.muted
                            });
                            if ui.selectable_label(selected, label).clicked() {
                                picked = Some(*value);
                            }
                        }
                    },
                );
            });
        });
    picked
}
