//! Design tokens, assets, settings and view models.
//!
//! Everything the UI needs that is *not* a deeply nested render chain lives
//! here, so it can carry unit tests. `kirikumo-views` keeps only the views:
//! `rustc` overflows its stack expanding `#[test]` in a crate that also holds
//! the toolkit's builder chains, so the split is load-bearing, not cosmetic
//! (`AGENTS.md` rule 6).
//!
//! The three modules worth reading first are [`nav`], which turns the
//! cluster's catalogue into the sidebar's tree; [`table`], which turns an
//! object into a row; [`detail`], which turns one into the right panel; and
//! [`gitops`], which reads Argo CD's native Application status.
//! All three are functions of the object's JSON, which is what lets a custom
//! resource render with no code written for it (rule 8).

// The strings live in the workspace's own `locales/`, shared by every crate
// that shows one. English is the fallback, so a key a translator has not
// reached yet still renders as words.
rust_i18n::i18n!("../../locales", fallback = "en");

pub mod actions;
pub mod assets;
pub mod detail;
pub mod fetch;
pub mod filter;
pub mod gitops;
pub mod i18n;
pub mod layout;
pub mod logging;
pub mod logs;
pub mod nav;
pub mod palette;
pub mod paths;
pub mod settings;
pub mod table;
pub mod terminal;
pub mod theme;
pub mod time;

pub use actions::Pending;
pub use assets::Assets;
pub use detail::{Condition, Fact, Overview};
pub use fetch::Fetch;
pub use filter::Filter;
pub use layout::{HEADER_HEIGHT, Layout, Panel, TRAFFIC_LIGHT_INSET};
pub use nav::{NavRow, Section, Subsection};
pub use palette::{Action, Command, Entry, Here};
pub use paths::Paths;
pub use settings::{AppSettings, Appearance};
pub use table::{Column, ColumnSet, Row, Width};
pub use terminal::Screen;
pub use theme::{Mode, Tokens};
