//! Which of the two palettes the window draws in, and where that choice is remembered.
//!
//! Kept apart from [`crate::theme`], which holds the colours themselves and is compiled only
//! with the `gui` feature: the *choice* is plain data with a plain file behind it, so it is
//! built and tested on a CI box with no display, exactly like the language preference it sits
//! beside in [`crate::i18n`].
//!
//! Dark is the default. A vibrometer console is read across a test cell, often in a bay with
//! the lights down and the laser running, and the shipped instrument has always been a dark
//! panel; an operator who wants the bright one says so once and the choice is kept.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::i18n::{Label, Lang};

/// The palette the window is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Theme {
    /// The dark console: the shipped look, and what a first launch shows.
    #[default]
    Dark,
    /// The light panel, for a brightly lit bench or a printed screenshot.
    Light,
}

impl Theme {
    /// Both themes, in the order the switch draws them.
    pub const ALL: [Self; 2] = [Self::Light, Self::Dark];

    /// The tag stored in the preference file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Parse a stored tag. Anything unrecognised is dark, by policy — the same rule the
    /// language store follows for Chinese.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text.trim().eq_ignore_ascii_case("light") {
            Self::Light
        } else {
            Self::Dark
        }
    }

    /// The label of this theme's own button: 浅色 / 深色, Light / Dark.
    #[must_use]
    pub const fn label(self) -> Label {
        match self {
            Self::Dark => Label::ThemeDark,
            Self::Light => Label::ThemeLight,
        }
    }

    /// The name of this theme in `lang`.
    #[must_use]
    pub const fn name(self, lang: Lang) -> &'static str {
        lang.t(self.label())
    }

    /// Whether this is the dark palette, for the egui `Visuals` flag of the same name.
    #[must_use]
    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    /// The other theme — what the toggle switches to.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// File name of the theme preference inside the QuickVib state directory.
pub const THEME_FILE_NAME: &str = "ui-theme.txt";

/// Where the window remembers the operator's theme choice between launches.
///
/// One line, one word, in the same per-user state directory as `ui-language.txt`
/// (`%LOCALAPPDATA%\QuickVib\` on Windows, `$XDG_STATE_HOME/quickvib/` elsewhere). Every
/// failure to read or write it is silent: a locked-down host that cannot keep the preference
/// simply opens dark, which is the default anyway.
#[derive(Debug, Clone)]
pub struct ThemeStore {
    path: PathBuf,
}

impl ThemeStore {
    /// A store that keeps its record in `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join(THEME_FILE_NAME),
        }
    }

    /// A store in the platform state directory, when one can be resolved.
    #[must_use]
    pub fn discover() -> Option<Self> {
        quickvib_project::last_project::state_dir().map(Self::new)
    }

    /// The full path of the record.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stored preference, or `None` when nothing has been stored yet.
    #[must_use]
    pub fn read(&self) -> Option<Theme> {
        std::fs::read_to_string(&self.path)
            .ok()
            .map(|text| Theme::parse(&text))
    }

    /// Remember `theme`. Returns whether the record could be written.
    pub fn write(&self, theme: Theme) -> bool {
        let Some(parent) = self.path.parent() else {
            return false;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
        std::fs::write(&self.path, theme.as_str()).is_ok()
    }
}

/// The theme the window opens in: the stored preference when there is one, dark otherwise.
#[must_use]
pub fn startup_theme(store: Option<&ThemeStore>) -> Theme {
    store.and_then(ThemeStore::read).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn dark_is_the_shipped_default() {
        assert_eq!(Theme::default(), Theme::Dark);
        assert_eq!(startup_theme(None), Theme::Dark);
        assert!(Theme::default().is_dark());
    }

    #[test]
    fn a_stored_tag_round_trips_and_anything_else_is_dark() {
        for theme in Theme::ALL {
            assert_eq!(Theme::parse(theme.as_str()), theme);
            assert_eq!(Theme::parse(&theme.to_string()), theme);
        }
        assert_eq!(Theme::parse("LIGHT"), Theme::Light);
        assert_eq!(Theme::parse(" light\n"), Theme::Light);
        assert_eq!(Theme::parse(""), Theme::Dark);
        assert_eq!(Theme::parse("solarized"), Theme::Dark);
    }

    #[test]
    fn the_toggle_flips_between_exactly_two_themes() {
        assert_eq!(Theme::Dark.toggled(), Theme::Light);
        assert_eq!(Theme::Light.toggled(), Theme::Dark);
        for theme in Theme::ALL {
            assert_eq!(theme.toggled().toggled(), theme);
        }
    }

    #[test]
    fn both_themes_are_named_in_both_languages() {
        assert_eq!(Theme::Light.name(Lang::Zh), "浅色");
        assert_eq!(Theme::Dark.name(Lang::Zh), "深色");
        assert_eq!(Theme::Light.name(Lang::En), "Light");
        assert_eq!(Theme::Dark.name(Lang::En), "Dark");
    }

    #[test]
    fn the_preference_round_trips_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = ThemeStore::new(dir.path());
        assert_eq!(store.read(), None);
        assert_eq!(startup_theme(Some(&store)), Theme::Dark);

        assert!(store.write(Theme::Light));
        assert_eq!(store.read(), Some(Theme::Light));
        assert_eq!(startup_theme(Some(&store)), Theme::Light);

        assert!(store.write(Theme::Dark));
        assert_eq!(startup_theme(Some(&store)), Theme::Dark);
        assert!(store.path().ends_with(THEME_FILE_NAME));
    }

    #[test]
    fn the_theme_is_remembered_beside_the_language_and_not_instead_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let langs = crate::i18n::LangStore::new(dir.path());
        let themes = ThemeStore::new(dir.path());
        assert!(langs.write(Lang::En));
        assert!(themes.write(Theme::Light));

        assert_ne!(langs.path(), themes.path());
        assert_eq!(langs.path().parent(), themes.path().parent());
        assert_eq!(crate::i18n::startup_lang(Some(&langs)), Lang::En);
        assert_eq!(startup_theme(Some(&themes)), Theme::Light);
    }

    #[test]
    fn an_unwritable_store_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("occupied");
        std::fs::write(&file, "not a directory").unwrap();
        let store = ThemeStore::new(file.join("state"));
        assert!(!store.write(Theme::Light));
        assert_eq!(startup_theme(Some(&store)), Theme::Dark);
    }
}
