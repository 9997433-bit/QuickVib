//! The CJK font the window embeds.
//!
//! egui ships Latin faces only. Drawing 录制中 with them produces a row of tofu boxes, which
//! on a factory floor is indistinguishable from a broken instrument, so QuickVib carries its
//! own font rather than hoping the host has one: a UTS bench PC is not a machine anyone gets
//! to `apt install fonts-noto-cjk` on.
//!
//! What is committed is a subset — Latin, the symbols the panel uses, and the 3755 hanzi of
//! GB 2312 level 1 — built by `assets/build-font.py`, which documents exactly how to
//! regenerate it. The bytes are compiled in only with the `gui` feature (and in this crate's
//! own tests), so the headless UTS binary does not carry a megabyte of glyphs it will never
//! draw.
//!
//! Noto Sans SC, Copyright 2014-2021 Adobe, SIL Open Font License 1.1 (`assets/OFL.txt`).

/// The embedded font, as TrueType bytes.
#[cfg(any(feature = "gui", test))]
pub(crate) const CJK_FONT: &[u8] = include_bytes!("../assets/NotoSansSC-Regular-subset.ttf");

/// The name the window registers the embedded font under.
#[cfg(feature = "gui")]
pub(crate) const CJK_FONT_NAME: &str = "noto-sans-sc";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::BTreeSet;

    use quickvib_core::{BackendKind, SampleUnit, ScpiError};
    use quickvib_engine::State;

    use super::CJK_FONT;
    use crate::controller::{Action, ActionError, ApplyOutcome, RestartField};
    use crate::form::Issue;
    use crate::i18n::{self, Label, Lang};

    /// Every string the window can draw that this crate produces, in both languages.
    ///
    /// A label whose characters are missing from the embedded font is a row of tofu on the
    /// instrument, and nobody sees it until someone opens the window on a machine with a
    /// display. Collecting the whole vocabulary here turns that into a `cargo test` failure.
    fn every_string() -> Vec<String> {
        let mut strings = Vec::new();
        for lang in [Lang::Zh, Lang::En] {
            strings.push(lang.endonym().to_owned());
            strings.extend(Label::ALL.iter().map(|label| lang.t(*label).to_owned()));
            strings.extend(
                State::all()
                    .iter()
                    .map(|state| i18n::state_label(lang, *state).to_owned()),
            );
            strings.extend(
                [BackendKind::Mock, BackendKind::Tcp, BackendKind::M300]
                    .iter()
                    .map(|backend| i18n::backend_label(lang, *backend).to_owned()),
            );
            strings.extend(
                SampleUnit::all()
                    .iter()
                    .map(|unit| i18n::unit_label(lang, *unit)),
            );
            strings.extend(
                ScpiError::all()
                    .iter()
                    .map(|error| i18n::scpi_message(lang, *error).to_owned()),
            );
            strings.extend(
                [
                    "name",
                    "device.sampleRateHz",
                    "device.lpfHz",
                    "device.highPassHz",
                    "device.velocityRange",
                    "device.displacementRange",
                    "device.accelerationRange",
                    "device.port",
                    "recording.durationSeconds",
                    "server.scpiPort",
                    "export.directory",
                    "project",
                ]
                .iter()
                .map(|field| i18n::field_label(lang, field).to_owned()),
            );
            strings.extend(
                [
                    Issue::Empty,
                    Issue::NotANumber {
                        input: "x".to_owned(),
                    },
                    Issue::NotPositive { value: -1.0 },
                    Issue::Negative { value: -1.0 },
                    Issue::BadPort {
                        input: "0".to_owned(),
                    },
                    Issue::DuplicatePort,
                    Issue::AboveLowPass { lpf_hz: 50_000.0 },
                    Issue::DurationTooLong {
                        max_seconds: 3600.0,
                    },
                    Issue::CaptureTooLarge {
                        max_bytes: 268_435_456,
                    },
                    Issue::RunInFlight,
                ]
                .iter()
                .filter_map(|issue| i18n::issue_text(lang, issue)),
            );
            strings.extend(
                [
                    Action::Load("a.proj".into()),
                    Action::Save("a.proj".into()),
                    Action::Start,
                    Action::Export,
                ]
                .iter()
                .map(|action| action.failed_text(lang)),
            );
            strings.push(
                ActionError::Refused {
                    action: Action::Start,
                    error: ScpiError::SettingsConflict,
                }
                .localized(lang),
            );
            strings.push(ActionError::NoProjectFile.localized(lang));
            strings.push(ActionError::BlankName.localized(lang));
            strings.push(
                ApplyOutcome {
                    restart_required: vec![
                        RestartField::Backend,
                        RestartField::DevicePort,
                        RestartField::ScpiPort,
                        RestartField::SampleRate,
                        RestartField::DataType,
                    ],
                }
                .restart_note(lang)
                .unwrap(),
            );
            strings.push(i18n::rejected_fields(lang, 3));
            strings.push(i18n::saved(lang, std::path::Path::new("a.proj")));
            strings.push(i18n::loaded(lang, "a.proj"));
            strings.push(i18n::wrote(lang, std::path::Path::new("a.csv")));
            strings.push(i18n::seconds(lang, 1.5));
            strings.push(i18n::refusal(lang, "x", ScpiError::TimeoutError));
        }
        strings.push(i18n::frequency(100_000.0));
        strings.push(i18n::measurement(
            1.0,
            Some(SampleUnit::AccelerationMPerSec2),
        ));
        strings
    }

    #[test]
    fn the_embedded_font_covers_every_string_the_window_can_draw() {
        let covered = covered_codepoints(CJK_FONT);
        let mut missing = BTreeSet::new();
        for text in every_string() {
            for c in text.chars() {
                if !covered.contains(&(c as u32)) {
                    missing.insert(c);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "the embedded font has no glyph for {missing:?}; \
             rerun crates/quickvib-ui/assets/build-font.py after adding the character"
        );
    }

    #[test]
    fn the_embedded_font_covers_everyday_chinese_beyond_the_interface() {
        // Project names and file paths are the operator's words, not ours, so the subset has
        // to reach past the label table into everyday characters.
        let covered = covered_codepoints(CJK_FONT);
        for c in "电机测试振动位移主轴转子轴承样品编号第三次试验".chars() {
            assert!(covered.contains(&(c as u32)), "no glyph for {c}");
        }
    }

    /// Every code point the font's `cmap` maps to a real glyph.
    ///
    /// A hand-rolled format-4 reader rather than a font crate: this workspace ships two
    /// third-party dependencies and a font parser is not going to be the third, and the whole
    /// job is one well-documented table.
    fn covered_codepoints(font: &[u8]) -> BTreeSet<u32> {
        let cmap = table(font, b"cmap").expect("the font has a cmap table");
        let subtable_count = u16::from_be_bytes([cmap[2], cmap[3]]) as usize;

        let mut covered = BTreeSet::new();
        let mut found_format_4 = false;
        for index in 0..subtable_count {
            let record = 4 + index * 8;
            let offset = u32::from_be_bytes([
                cmap[record + 4],
                cmap[record + 5],
                cmap[record + 6],
                cmap[record + 7],
            ]) as usize;
            let subtable = &cmap[offset..];
            if u16::from_be_bytes([subtable[0], subtable[1]]) != 4 {
                continue;
            }
            found_format_4 = true;
            read_format_4(subtable, &mut covered);
        }
        assert!(found_format_4, "no format 4 cmap subtable");
        covered
    }

    /// The bytes of one table of a TrueType file.
    fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
        let count = u16::from_be_bytes([font[4], font[5]]) as usize;
        for index in 0..count {
            let record = 12 + index * 16;
            if &font[record..record + 4] != tag {
                continue;
            }
            let offset =
                u32::from_be_bytes(font[record + 8..record + 12].try_into().ok()?) as usize;
            let length =
                u32::from_be_bytes(font[record + 12..record + 16].try_into().ok()?) as usize;
            return Some(&font[offset..offset + length]);
        }
        None
    }

    /// The segmented BMP mapping, as specified in the OpenType `cmap` chapter.
    fn read_format_4(subtable: &[u8], covered: &mut BTreeSet<u32>) {
        let be = |at: usize| u16::from_be_bytes([subtable[at], subtable[at + 1]]);
        let segments = be(6) as usize / 2;
        let end_codes = 14;
        let start_codes = end_codes + segments * 2 + 2;
        let deltas = start_codes + segments * 2;
        let range_offsets = deltas + segments * 2;

        for segment in 0..segments {
            let end = be(end_codes + segment * 2);
            let start = be(start_codes + segment * 2);
            let delta = be(deltas + segment * 2);
            let range_offset = be(range_offsets + segment * 2);
            if start > end {
                continue;
            }
            for code in start..=end {
                if code == 0xFFFF {
                    continue;
                }
                let glyph = if range_offset == 0 {
                    code.wrapping_add(delta)
                } else {
                    let at = range_offsets
                        + segment * 2
                        + range_offset as usize
                        + (code - start) as usize * 2;
                    if at + 1 >= subtable.len() {
                        continue;
                    }
                    let glyph = be(at);
                    if glyph == 0 {
                        0
                    } else {
                        glyph.wrapping_add(delta)
                    }
                };
                if glyph != 0 {
                    covered.insert(u32::from(code));
                }
            }
        }
    }
}
