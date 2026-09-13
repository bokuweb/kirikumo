//! What the window remembers: `~/.kirikumo/app.json`.
//!
//! The only thing this app writes (`AGENTS.md` rule 10). What is worth
//! remembering is where the reader *was* — the context, the kind and the
//! namespace — because reopening a viewer on the same screen is the
//! difference between a tool and a website.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One reader override for a table column.
///
/// Entries are stored in display order. A missing width keeps the built-in
/// `kubectl get` proportion, while `hidden` removes only that heading and its
/// already-formatted cell; no Kubernetes payload is persisted with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnPreference {
    /// The heading used to match this preference after discovery runs again.
    pub name: String,
    /// A reader-chosen width in pixels, or the built-in width.
    pub width: Option<f32>,
    /// Whether this column is omitted from the table.
    pub hidden: bool,
}

impl ColumnPreference {
    /// A visible column, optionally fixed to a width in pixels.
    pub fn shown(name: impl Into<String>, width: Option<f32>) -> Self {
        Self {
            name: name.into(),
            width,
            hidden: false,
        }
    }

    /// A column omitted from the table.
    pub fn hidden(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            width: None,
            hidden: true,
        }
    }
}

/// The window's own settings.
///
/// `deny_unknown_fields` so a typo in a hand-edited file is reported rather
/// than silently ignored, and `default` so a file from an older build still
/// loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppSettings {
    /// Light, dark, or whatever the OS says.
    pub appearance: Appearance,
    /// Whether the navigation column is showing.
    pub sidebar_open: bool,
    /// Its width, kept while it is closed.
    pub sidebar_width: f32,
    /// Whether the detail column is showing.
    pub right_panel_open: bool,
    /// Its width, kept while it is closed.
    pub right_panel_width: f32,
    /// BCP-47 tag; `None` follows the system locale.
    pub locale: Option<String>,
    /// The kubeconfig context that was on screen, restored on launch. `None`
    /// opens on whatever the kubeconfig says is current.
    pub last_context: Option<String>,
    /// The kind that was on screen, as `Deployment.apps`.
    pub last_resource: Option<String>,
    /// The namespace that was selected; `None` is every namespace.
    pub last_namespace: Option<String>,
    /// The sidebar groups that are folded away, by name.
    pub collapsed_groups: Vec<String>,
    /// Per-resource table column order, visibility and reader-chosen widths.
    pub table_columns: BTreeMap<String, Vec<ColumnPreference>>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            // The defaults in docs/ui.md §2. The right panel starts open: it
            // is the reading pane, and there is something to read the moment
            // a row is picked.
            sidebar_open: true,
            sidebar_width: 250.0,
            right_panel_open: true,
            right_panel_width: 420.0,
            locale: None,
            last_context: None,
            // Pods is where a person looks first, and it is the one kind
            // every cluster has.
            last_resource: Some("Pod".to_string()),
            last_namespace: None,
            collapsed_groups: Vec::new(),
            table_columns: BTreeMap::new(),
        }
    }
}

/// The theme choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Always light.
    Light,
    /// Always dark.
    Dark,
    /// Whatever the window's appearance is, and follow it when it changes.
    #[default]
    System,
}

impl Appearance {
    /// Dark, then light, then the system's, then dark again.
    pub fn next(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::System,
            Self::System => Self::Dark,
        }
    }
}

/// Read a settings file, falling back to defaults.
///
/// A missing file is normal (first run). A *corrupt* file is not silently
/// replaced: we log loudly and use defaults for this session, so a syntax
/// error in a hand-edited file never destroys the rest of the configuration.
pub fn load<T: Default + serde::de::DeserializeOwned>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(
                    path = %path.display(),
                    %error,
                    "settings file is not valid; using defaults for this session without \
                     overwriting the file"
                );
                T::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => T::default(),
        Err(error) => {
            tracing::error!(path = %path.display(), %error, "could not read settings");
            T::default()
        }
    }
}

/// Write settings atomically, so a crash mid-write cannot truncate the file.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(value)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reading_pane_starts_open_and_the_window_starts_on_pods() {
        let settings = AppSettings::default();
        assert!(settings.sidebar_open);
        assert!(settings.right_panel_open);
        assert_eq!(settings.last_resource.as_deref(), Some("Pod"));
        assert_eq!(settings.last_namespace, None);
    }

    #[test]
    fn a_file_from_an_older_build_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.json");
        std::fs::write(&path, r#"{"sidebar_open": false}"#).unwrap();
        let settings: AppSettings = load(&path);
        assert!(!settings.sidebar_open);
        assert_eq!(settings.right_panel_width, 420.0);
    }

    #[test]
    fn a_corrupt_file_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.json");
        std::fs::write(&path, "{ not json").unwrap();
        let settings: AppSettings = load(&path);
        assert_eq!(settings, AppSettings::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.json");
        let settings = AppSettings {
            last_context: Some("kind-dev".into()),
            last_resource: Some("Deployment.apps".into()),
            last_namespace: Some("shop".into()),
            sidebar_width: 300.0,
            ..AppSettings::default()
        };
        save(&path, &settings).unwrap();
        assert_eq!(load::<AppSettings>(&path), settings);
    }

    #[test]
    fn table_column_preferences_round_trip_without_any_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.json");
        let mut settings = AppSettings::default();
        settings.table_columns.insert(
            "Deployment.apps".into(),
            vec![
                ColumnPreference::shown("NAME", Some(260.0)),
                ColumnPreference::hidden("IMAGES"),
                ColumnPreference::shown("READY", None),
            ],
        );

        save(&path, &settings).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: AppSettings = load(&path);

        assert_eq!(loaded, settings);
        assert!(text.contains("table_columns"));
        assert!(!text.contains("resourceVersion"));
    }

    #[test]
    fn the_appearance_control_cycles_and_comes_back_round() {
        let mut appearance = Appearance::Dark;
        for _ in 0..3 {
            appearance = appearance.next();
        }
        assert_eq!(appearance, Appearance::Dark);
    }
}
