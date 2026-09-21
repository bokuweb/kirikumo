//! Design tokens.
//!
//! The single source of truth for colour, radius and motion is
//! `assets/themes/*.json` — Ginka's own files, byte for byte, because these
//! views will one day render inside its window (`docs/roadmap.md` K5). Views
//! resolve everything through the `Tokens` global; no view hardcodes a
//! colour, a radius or a duration (`AGENTS.md` rule 5).

use crate::settings::Appearance;
use crate::terminal::Colour;
use gpui::{App, Global, Hsla, Rgba, WindowAppearance};
use gpui_component::highlighter::HighlightTheme;
use kirikumo_kube::Level;
use serde::{Deserialize, Deserializer};
use std::{sync::Arc, time::Duration};

const DARK: &str = include_str!("../../../assets/themes/dark.json");
const LIGHT: &str = include_str!("../../../assets/themes/light.json");

/// Light or dark, once the user's choice has been resolved against the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Light.
    Light,
    /// Dark.
    Dark,
}

impl Mode {
    /// Resolve the user's choice against the window's actual appearance.
    ///
    /// `Appearance::System` is not a third theme: it is a deferral to the OS,
    /// and the OS answer can change while the app is running.
    pub fn resolve(choice: Appearance, system: WindowAppearance) -> Self {
        match choice {
            Appearance::Light => Self::Light,
            Appearance::Dark => Self::Dark,
            Appearance::System => match system {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            },
        }
    }
}

/// The complete token set.
///
/// Fields are exhaustive rather than added on demand: the palette is a
/// contract themes are authored against, and a token that appears only when
/// some view needs it cannot be validated in `assets/themes/`.
#[derive(Debug, Clone, Deserialize)]
pub struct Tokens {
    /// The theme's own name.
    pub name: String,
    /// Which side of the light/dark line it is on.
    pub appearance: ThemeAppearance,
    /// Every colour.
    pub colors: Colors,
    /// Every radius.
    pub radius: Radii,
    /// Every duration.
    pub duration_ms: Durations,
}

/// What a theme file says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeAppearance {
    /// Light.
    Light,
    /// Dark.
    Dark,
}

/// Every colour the app may paint.
///
/// `deny_unknown_fields` plus non-optional members means a theme file that
/// misspells or omits a token fails to parse instead of rendering a black
/// hole somewhere in the UI.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    /// The window base, translucent.
    #[serde(rename = "bg.window", deserialize_with = "hex")]
    pub bg_window: Hsla,
    /// The left column.
    #[serde(rename = "bg.sidebar", deserialize_with = "hex")]
    pub bg_sidebar: Hsla,
    /// Cards and fields.
    #[serde(rename = "bg.surface", deserialize_with = "hex")]
    pub bg_surface: Hsla,
    /// Popovers and menus.
    #[serde(rename = "bg.raised", deserialize_with = "hex")]
    pub bg_raised: Hsla,
    /// A terminal pane.
    #[serde(rename = "bg.terminal", deserialize_with = "hex")]
    pub bg_terminal: Hsla,
    /// Panel separators.
    #[serde(rename = "border.subtle", deserialize_with = "hex")]
    pub border_subtle: Hsla,
    /// A focused input, a selected row.
    #[serde(rename = "border.strong", deserialize_with = "hex")]
    pub border_strong: Hsla,
    /// Titles and body.
    #[serde(rename = "text.primary", deserialize_with = "hex")]
    pub text_primary: Hsla,
    /// Subtitles and metadata.
    #[serde(rename = "text.secondary", deserialize_with = "hex")]
    pub text_secondary: Hsla,
    /// Timestamps, placeholders, and an object with nothing to say.
    #[serde(rename = "text.muted", deserialize_with = "hex")]
    pub text_muted: Hsla,
    /// Selection, links, focus.
    #[serde(deserialize_with = "hex")]
    pub accent: Hsla,
    /// Something on its way somewhere: pending, creating, terminating.
    #[serde(rename = "status.working", deserialize_with = "hex")]
    pub status_working: Hsla,
    /// Working, but not as intended.
    #[serde(rename = "status.attention", deserialize_with = "hex")]
    pub status_attention: Hsla,
    /// Healthy.
    #[serde(rename = "status.done", deserialize_with = "hex")]
    pub status_done: Hsla,
    /// Broken.
    #[serde(rename = "status.error", deserialize_with = "hex")]
    pub status_error: Hsla,
    /// Inline code, YAML, logs and collected command output.
    #[serde(rename = "code.bg", deserialize_with = "hex")]
    pub code_bg: Hsla,
}

/// Every radius.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Radii {
    /// The window's corners.
    pub window: f32,
    /// A card holding a group of controls.
    pub card: f32,
    /// A panel.
    pub panel: f32,
    /// A row, a chip.
    pub row: f32,
}

impl Radii {
    /// A control — a button, a text field — a step under a row: a control
    /// sits inside a card whose corner is the larger one, and matching it
    /// would read as a card in a card.
    pub fn control(&self) -> f32 {
        (self.row - 3.).max(2.)
    }
}

/// Motion durations: ~260 ms for layout, ~120 ms for feedback.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Durations {
    /// Hover and press.
    pub quick: u64,
    /// Layout and panel transitions.
    pub standard: u64,
}

impl Durations {
    /// Hover and press.
    pub fn quick(&self) -> Duration {
        Duration::from_millis(self.quick)
    }

    /// Layout and panel transitions.
    pub fn standard(&self) -> Duration {
        Duration::from_millis(self.standard)
    }
}

impl Tokens {
    /// The built-in theme for a mode.
    pub fn load(mode: Mode) -> Self {
        let source = match mode {
            Mode::Dark => DARK,
            Mode::Light => LIGHT,
        };
        // The themes are compiled in, so a parse failure is a build-time
        // authoring mistake and the tests below catch it.
        serde_json::from_str(source).expect("built-in theme is valid")
    }

    /// The installed tokens.
    pub fn global(cx: &App) -> &Tokens {
        cx.global::<Tokens>()
    }

    /// Install a built-in theme.
    pub fn install(mode: Mode, cx: &mut App) {
        cx.set_global(Tokens::load(mode));
    }

    /// The colours.
    pub fn colors(&self) -> &Colors {
        &self.colors
    }

    /// What the logo is painted in: white on the dark theme, navy on the
    /// light one. Not a token, because it is not a colour the rest of the
    /// window uses and a theme file should not have to name the logo.
    pub fn logo(&self) -> Hsla {
        match self.appearance {
            ThemeAppearance::Dark => gpui::white(),
            ThemeAppearance::Light => gpui::rgb(0x1E1B4B).into(),
        }
    }

    /// A table head separated from the glass without becoming an opaque band.
    pub fn table_head(&self) -> Hsla {
        match self.appearance {
            ThemeAppearance::Dark => gpui::black().opacity(0.35),
            ThemeAppearance::Light => self.colors.text_primary.opacity(0.07),
        }
    }
}

impl Global for Tokens {}

/// Parse `#RRGGBB` or `#RRGGBBAA`.
///
/// Alpha lives in the token rather than at the call site: the sidebar's
/// translucency is a property of the theme, and a view that re-applies
/// opacity on top of it drifts out of spec.
fn hex<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Hsla, D::Error> {
    use serde::de::Error as _;
    let raw = String::deserialize(deserializer)?;
    parse_hex(&raw).map_err(D::Error::custom)
}

/// Parse `RRGGBB` or `RRGGBBAA`, with or without a leading `#`.
pub fn parse_hex(raw: &str) -> Result<Hsla, String> {
    let digits = raw.strip_prefix('#').unwrap_or(raw);
    let (rgb, alpha) = match digits.len() {
        6 => (digits, 0xFF),
        8 => (
            &digits[..6],
            u8::from_str_radix(&digits[6..], 16).map_err(|e| e.to_string())?,
        ),
        other => {
            return Err(format!(
                "expected #RRGGBB or #RRGGBBAA, got {other} digits in {raw:?}"
            ));
        }
    };
    let value = u32::from_str_radix(rgb, 16).map_err(|e| e.to_string())?;
    Ok(Rgba {
        r: ((value >> 16) & 0xFF) as f32 / 255.0,
        g: ((value >> 8) & 0xFF) as f32 / 255.0,
        b: (value & 0xFF) as f32 / 255.0,
        a: alpha as f32 / 255.0,
    }
    .into())
}

/// An exact colour as GPUI holds it.
fn rgb_hsla(red: u8, green: u8, blue: u8) -> Hsla {
    Rgba {
        r: red as f32 / 255.,
        g: green as f32 / 255.,
        b: blue as f32 / 255.,
        a: 1.,
    }
    .into()
}

impl Colors {
    /// The colour a health mark is painted in.
    ///
    /// The one place [`Level`] becomes a colour. `kirikumo-kube` names the
    /// token (`Level::token`) without knowing what a colour is; this resolves
    /// that name, and the test below is what keeps the two from drifting.
    pub fn health(&self, level: Level) -> Hsla {
        match level {
            Level::Ok => self.status_done,
            Level::Working => self.status_working,
            Level::Attention => self.status_attention,
            Level::Error => self.status_error,
            Level::Unknown => self.text_muted,
        }
    }

    /// What a terminal's colour is in this theme.
    ///
    /// The eight ANSI colours and their bright halves are resolved against
    /// the theme's own palette rather than to fixed hexes: a terminal that
    /// hardcoded them would clash with every theme but the one it was written
    /// on. The 216-colour cube and the greys are computed the way xterm
    /// computes them, and what a program asked for exactly — a truecolour
    /// escape — it gets exactly.
    pub fn terminal(&self, colour: Colour) -> Hsla {
        match colour {
            Colour::Rgb(red, green, blue) => rgb_hsla(red, green, blue),
            Colour::Named(index @ 0..=15) => match index % 8 {
                0 => self.text_muted,
                1 => self.status_error,
                2 => self.status_done,
                3 => self.status_attention,
                4 => self.accent,
                5 => self.status_working,
                6 => self.text_secondary,
                _ => self.text_primary,
            },
            Colour::Named(index @ 16..=231) => {
                let index = index - 16;
                let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
                rgb_hsla(level(index / 36), level((index / 6) % 6), level(index % 6))
            }
            Colour::Named(index) => {
                let grey = 8 + (index - 232) * 10;
                rgb_hsla(grey, grey, grey)
            }
        }
    }

    /// A fully transparent fill, for surfaces that should show the window's
    /// glass rather than paint over it.
    fn transparent_surface(&self) -> Hsla {
        let mut color = self.bg_surface;
        color.a = 0.0;
        color
    }

    /// A row under the pointer: the accent at a fraction of itself rather
    /// than a grey fill, because a grey fill over a translucent window is
    /// what turns glass into cardboard.
    pub fn row_hover(&self) -> Hsla {
        self.accent.opacity(0.14)
    }

    /// The row you are on.
    pub fn row_active(&self) -> Hsla {
        self.accent.opacity(0.22)
    }

    /// A raised control the pointer is over.
    pub fn surface_hover(&self) -> Hsla {
        self.bg_raised.opacity((self.bg_raised.a + 0.14).min(1.0))
    }
}

/// Build the editor palette from the toolkit's matching syntax theme, then
/// replace its surfaces with Kirikumo's tokens.
fn editor_highlight_theme(mode: Mode, tokens: &Tokens) -> Arc<HighlightTheme> {
    let (appearance, base) = match mode {
        Mode::Dark => (
            gpui_component::ThemeMode::Dark,
            HighlightTheme::default_dark(),
        ),
        Mode::Light => (
            gpui_component::ThemeMode::Light,
            HighlightTheme::default_light(),
        ),
    };
    let mut theme = (*base).clone();
    theme.name = format!("{} editor", tokens.name);
    theme.appearance = appearance;
    theme.style.editor_background = Some(tokens.colors.code_bg);
    theme.style.editor_foreground = Some(tokens.colors.text_primary);
    theme.style.editor_gutter_background = Some(tokens.colors.code_bg);
    theme.style.editor_line_number = Some(tokens.colors.text_muted);
    theme.style.editor_active_line_number = Some(tokens.colors.text_secondary);
    theme.style.editor_invisible = Some(tokens.colors.text_muted.opacity(0.45));
    theme.style.editor_active_line = Some(tokens.colors.row_hover());
    Arc::new(theme)
}

/// Push our tokens into `gpui-component`'s theme.
///
/// The toolkit's components resolve their own palette through `cx.theme()`,
/// so a tooltip drawn by the library and a row drawn by us have to agree.
/// Mapping once here is what keeps them from drifting; nothing else in the
/// app should touch `Theme::global_mut`.
pub fn apply(mode: Mode, cx: &mut App) {
    Tokens::install(mode, cx);
    let table_head = Tokens::global(cx).table_head();
    let highlight_theme = editor_highlight_theme(mode, Tokens::global(cx));
    let tokens = *Tokens::global(cx).colors();
    let radii = Tokens::global(cx).radius;

    let theme = gpui_component::Theme::global_mut(cx);
    theme.mode = match mode {
        Mode::Dark => gpui_component::ThemeMode::Dark,
        Mode::Light => gpui_component::ThemeMode::Light,
    };

    theme.colors.background = tokens.bg_window;
    theme.colors.foreground = tokens.text_primary;
    theme.colors.muted = tokens.bg_surface;
    theme.colors.muted_foreground = tokens.text_secondary;
    theme.colors.border = tokens.border_subtle;
    theme.colors.accent = tokens.row_active();
    theme.colors.accent_foreground = tokens.text_primary;
    theme.colors.input = tokens.bg_surface;
    theme.colors.ring = tokens.accent.opacity(0.45);
    theme.colors.selection = tokens.accent.opacity(0.32);
    theme.colors.caret = tokens.accent;

    theme.colors.secondary = tokens.bg_surface;
    theme.colors.secondary_foreground = tokens.text_primary;
    theme.colors.secondary_hover = tokens.surface_hover();
    theme.colors.secondary_active = tokens.surface_hover();

    theme.colors.scrollbar = tokens.transparent_surface();
    theme.colors.scrollbar_thumb = tokens.row_active();
    theme.colors.scrollbar_thumb_hover = tokens.surface_hover();

    theme.colors.sidebar = tokens.bg_sidebar;
    theme.colors.sidebar_foreground = tokens.text_primary;
    theme.colors.sidebar_border = tokens.border_subtle;
    theme.colors.sidebar_accent = tokens.row_active();
    theme.colors.sidebar_accent_foreground = tokens.text_primary;

    // Transparent, not `bg_window`. `Root` already paints the window's
    // translucent background; a second layer of the same colour composites
    // with it and the glass turns opaque.
    theme.colors.title_bar = gpui::transparent_black();
    theme.colors.title_bar_border = tokens.border_subtle;
    theme.colors.window_border = tokens.border_subtle;

    // A placeholder bar: the row tint, so it reads as the ghost of a row.
    theme.colors.skeleton = tokens.row_hover();
    theme.colors.popover = tokens.bg_raised;
    theme.colors.popover_foreground = tokens.text_primary;
    theme.colors.list = tokens.transparent_surface();
    theme.colors.list_hover = tokens.row_hover();
    theme.colors.list_active = tokens.row_active();
    theme.colors.list_active_border = tokens.accent;

    theme.colors.tab_bar = tokens.transparent_surface();
    theme.colors.tab = tokens.transparent_surface();
    theme.colors.tab_active = tokens.bg_raised;
    theme.colors.tab_foreground = tokens.text_secondary;
    theme.colors.tab_active_foreground = tokens.text_primary;

    theme.colors.link = tokens.accent;
    theme.colors.link_hover = tokens.accent.opacity(0.85);
    theme.colors.link_active = tokens.accent.opacity(0.7);
    theme.colors.table = tokens.transparent_surface();
    theme.colors.table_head = table_head;
    theme.colors.table_head_foreground = tokens.text_secondary;
    theme.colors.table_foot = table_head;
    theme.colors.table_foot_foreground = tokens.text_secondary;
    theme.colors.table_even = tokens.transparent_surface();
    theme.colors.table_row_border = tokens.border_subtle;
    theme.colors.table_hover = tokens.row_hover();
    theme.colors.table_active = tokens.row_active();

    theme.colors.primary = tokens.accent;
    theme.colors.primary_foreground = tokens.bg_window;
    theme.colors.primary_hover = tokens.accent.opacity(0.85);
    theme.colors.primary_active = tokens.accent.opacity(0.7);
    theme.colors.danger = tokens.status_error;
    theme.colors.success = tokens.status_done;

    theme.radius = gpui::px(radii.control());
    theme.radius_lg = gpui::px(radii.panel);
    // The base sizes every toolkit control inherits: 13 px sans and 12 px
    // mono, a step under the defaults, which is what the reference reads at.
    theme.font_size = gpui::px(13.);
    theme.mono_font_size = gpui::px(12.);
    theme.highlight_theme = highlight_theme;

    // `Root` and several components paint from the derived semantic tokens
    // rather than from `colors`. Without regenerating them the window keeps
    // the toolkit's opaque default background, which cancels the glass.
    theme.tokens = (&theme.colors).into();
    gpui_component::Theme::sync_base(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_built_in_themes_parse() {
        for mode in [Mode::Dark, Mode::Light] {
            let tokens = Tokens::load(mode);
            assert!(!tokens.name.is_empty());
            assert_eq!(tokens.radius.window, 12.0);
        }
    }

    #[test]
    fn a_terminals_colours_come_from_the_theme_or_from_xterm() {
        let colors = Tokens::load(Mode::Dark).colors;
        // Red, and bright red, are the theme's error colour.
        assert_eq!(colors.terminal(Colour::Named(1)), colors.status_error);
        assert_eq!(colors.terminal(Colour::Named(9)), colors.status_error);
        // The cube: 16 is black, 231 is white, 196 is pure red.
        assert_eq!(colors.terminal(Colour::Named(16)), rgb_hsla(0, 0, 0));
        assert_eq!(colors.terminal(Colour::Named(231)), rgb_hsla(255, 255, 255));
        assert_eq!(colors.terminal(Colour::Named(196)), rgb_hsla(255, 0, 0));
        // The greys: 232 is the darkest, 255 the lightest.
        assert_eq!(colors.terminal(Colour::Named(232)), rgb_hsla(8, 8, 8));
        assert_eq!(colors.terminal(Colour::Named(255)), rgb_hsla(238, 238, 238));
        // And an exact one is exact.
        assert_eq!(colors.terminal(Colour::Rgb(1, 2, 3)), rgb_hsla(1, 2, 3));
    }

    #[test]
    fn appearance_matches_the_file_it_came_from() {
        assert_eq!(Tokens::load(Mode::Dark).appearance, ThemeAppearance::Dark);
        assert_eq!(Tokens::load(Mode::Light).appearance, ThemeAppearance::Light);
    }

    #[test]
    fn every_surface_lets_the_blur_through() {
        let dark = Tokens::load(Mode::Dark);
        for (name, colour) in [
            ("bg.window", dark.colors.bg_window),
            ("bg.sidebar", dark.colors.bg_sidebar),
            ("bg.surface", dark.colors.bg_surface),
            ("bg.raised", dark.colors.bg_raised),
        ] {
            assert!(colour.a < 1.0, "{name} must let the blur through");
        }
    }

    #[test]
    fn system_appearance_defers_to_the_os_but_an_explicit_choice_wins() {
        assert_eq!(
            Mode::resolve(Appearance::System, WindowAppearance::Dark),
            Mode::Dark
        );
        assert_eq!(
            Mode::resolve(Appearance::System, WindowAppearance::VibrantLight),
            Mode::Light
        );
        assert_eq!(
            Mode::resolve(Appearance::Light, WindowAppearance::Dark),
            Mode::Light
        );
    }

    #[test]
    fn every_health_level_resolves_to_the_token_the_domain_named() {
        // `kirikumo-kube` names the token and knows nothing about colour;
        // this is the join, and the assertion that keeps the two honest.
        for mode in [Mode::Dark, Mode::Light] {
            let colors = Tokens::load(mode).colors;
            for level in [
                Level::Ok,
                Level::Working,
                Level::Attention,
                Level::Error,
                Level::Unknown,
            ] {
                let named = match level.token() {
                    "status.done" => colors.status_done,
                    "status.working" => colors.status_working,
                    "status.attention" => colors.status_attention,
                    "status.error" => colors.status_error,
                    "text.muted" => colors.text_muted,
                    other => panic!("{other} is not a token this theme has"),
                };
                assert_eq!(colors.health(level), named, "{level:?}");
            }
        }
    }

    #[test]
    fn the_five_health_colours_are_all_different_so_a_row_can_be_read_by_one() {
        let colors = Tokens::load(Mode::Dark).colors;
        let marks = [
            colors.health(Level::Ok),
            colors.health(Level::Working),
            colors.health(Level::Attention),
            colors.health(Level::Error),
            colors.health(Level::Unknown),
        ];
        for (index, one) in marks.iter().enumerate() {
            for other in &marks[index + 1..] {
                assert_ne!(one, other);
            }
        }
    }

    #[test]
    fn a_control_is_a_step_under_a_row_and_never_square() {
        let radii = Tokens::load(Mode::Dark).radius;
        assert!(radii.control() < radii.row);
        assert!(radii.control() >= 2.0);
    }

    #[test]
    fn the_sidebar_is_a_tint_over_the_glass_not_a_second_coat() {
        let dark = Tokens::load(Mode::Dark).colors;
        assert!(dark.bg_sidebar.a < 0.2);
        assert!(dark.bg_sidebar.l > dark.bg_window.l);
        assert!(dark.bg_window.a < 0.8);
    }

    #[test]
    fn the_logo_is_white_on_dark_and_navy_on_light() {
        assert!(Tokens::load(Mode::Dark).logo().l > 0.99);
        let navy = Tokens::load(Mode::Light).logo();
        assert!(navy.l < 0.25, "dark enough to read on the light glass");
        assert!(navy.s > 0.3, "blue, not grey");
    }

    #[test]
    fn yaml_highlighting_follows_the_selected_mode() {
        let dark_tokens = Tokens::load(Mode::Dark);
        let light_tokens = Tokens::load(Mode::Light);
        let dark = editor_highlight_theme(Mode::Dark, &dark_tokens);
        let light = editor_highlight_theme(Mode::Light, &light_tokens);

        assert_eq!(dark.appearance, gpui_component::ThemeMode::Dark);
        assert_eq!(light.appearance, gpui_component::ThemeMode::Light);
        assert_eq!(
            dark.style.editor_background,
            Some(dark_tokens.colors.code_bg)
        );
        assert_eq!(
            light.style.editor_background,
            Some(light_tokens.colors.code_bg)
        );
        assert_eq!(
            dark.style.editor_foreground,
            Some(dark_tokens.colors.text_primary)
        );
        assert_eq!(
            light.style.editor_foreground,
            Some(light_tokens.colors.text_primary)
        );
        assert_eq!(
            dark.style.editor_gutter_background,
            Some(dark_tokens.colors.code_bg)
        );
        assert_eq!(
            light.style.editor_gutter_background,
            Some(light_tokens.colors.code_bg)
        );
        assert_ne!(dark.style.syntax, light.style.syntax);
    }

    #[test]
    fn the_light_table_header_is_a_subtle_token_tint() {
        let dark = Tokens::load(Mode::Dark).table_head();
        let light_tokens = Tokens::load(Mode::Light);
        let light = light_tokens.table_head();

        assert!(light.a <= 0.1, "light glass must not get a dark grey band");
        assert_eq!(light.h, light_tokens.colors.text_primary.h);
        assert!(dark.a > light.a, "dark glass needs the stronger separator");
    }

    #[test]
    fn hex_is_strict_about_what_a_colour_looks_like() {
        assert!(parse_hex("d73a4a").is_ok());
        assert!(parse_hex("#0E0C12B8").is_ok());
        assert!(parse_hex("#FFF").is_err());
        assert!(parse_hex("#GGGGGG").is_err());
    }
}
