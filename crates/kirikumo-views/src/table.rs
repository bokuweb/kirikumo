//! The centre column: `docs/ui.md` §3.3.
//!
//! One virtualized table draws every kind. The columns and the cells come
//! from `kirikumo_ui::table`, which is where the per-kind knowledge lives and
//! where it is tested; this file is the drawing of it and nothing else.
//!
//! Rows are precomputed when a list lands and never during a scroll
//! (`AGENTS.md` rule 7): a namespace with four thousand pods costs four
//! thousand strings once, and whatever is on screen each frame.

use crate::store::{ObjectKey, Store, StoreEvent};
use chrono::Utc;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{Icon, StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use kirikumo_kube::ResourceKey;
use kirikumo_ui::assets::icon;
use kirikumo_ui::settings::ColumnPreference;
use kirikumo_ui::table::{self, ColumnSet, Row, Width};
use kirikumo_ui::{Filter, Tokens};
use std::collections::BTreeMap;

/// How tall a row is. One line, because this is a comparison list and not a
/// reading list (`docs/ui.md` §1.7).
const ROW_HEIGHT: Pixels = px(28.);

/// How tall the sticky header row is.
const HEAD_HEIGHT: Pixels = px(26.);

/// How wide the health mark's column is.
const MARK_WIDTH: Pixels = px(18.);

/// Emitted when the reader picks a row.
pub enum TableEvent {
    /// Open this object in the detail panel.
    Open(ObjectKey),
    /// Persist this resource's reader-chosen column arrangement.
    ColumnsChanged {
        /// A `Kind.group` resource identity.
        resource: String,
        /// Every current column in display order, hidden ones included.
        preferences: Vec<ColumnPreference>,
    },
}

impl EventEmitter<TableEvent> for ResourceTable {}

/// The table.
pub struct ResourceTable {
    store: Entity<Store>,
    kind: Option<ResourceKey>,
    namespace: Option<String>,
    /// The server-defined or built-in set before reader preferences.
    default_columns: ColumnSet,
    columns: ColumnSet,
    preferences: BTreeMap<String, Vec<ColumnPreference>>,
    showing_columns: bool,
    /// Every row of the list, sorted.
    rows: Vec<Row>,
    /// The indices of the rows the filter lets through, in table order.
    visible: Vec<usize>,
    filter: Filter,
    query: String,
    /// Which column the table is sorted by, and which way.
    sort: (usize, bool),
    /// The row that is open on the right, by its key.
    selected: Option<String>,
}

impl ResourceTable {
    /// A table over a store, showing nothing until told what to.
    pub fn new(
        store: Entity<Store>,
        preferences: BTreeMap<String, Vec<ColumnPreference>>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&store, |this, _, _: &StoreEvent, cx| this.rebuild(cx))
            .detach();
        let columns = ColumnSet::for_kind("", true, false);
        let sort = columns.default_sort();
        Self {
            store,
            kind: None,
            namespace: None,
            default_columns: columns.clone(),
            columns,
            preferences,
            showing_columns: false,
            rows: Vec::new(),
            visible: Vec::new(),
            filter: Filter::new(),
            query: String::new(),
            sort,
            selected: None,
        }
    }

    /// What is being listed.
    pub fn kind(&self) -> Option<&ResourceKey> {
        self.kind.as_ref()
    }

    /// Which namespace, or every one.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Whether the list is being fetched.
    pub fn is_loading(&self, cx: &App) -> bool {
        self.kind.as_ref().is_some_and(|kind| {
            self.store
                .read(cx)
                .list(kind, self.namespace.as_deref())
                .is_some_and(|list| list.is_loading())
        })
    }

    /// How many rows there are, and how many the filter lets through.
    pub fn counts(&self) -> (usize, usize) {
        (self.visible.len(), self.rows.len())
    }

    /// Open or close the column controls anchored over this table.
    pub fn toggle_columns(&mut self, cx: &mut Context<Self>) {
        self.showing_columns = !self.showing_columns;
        cx.notify();
    }

    /// List a kind, fetching it if it never has been.
    pub fn show(&mut self, kind: ResourceKey, cx: &mut Context<Self>) {
        if self.kind.as_ref() != Some(&kind) {
            // A new kind is a new set of columns, so the sort has to start
            // again: column 3 of Pods is not column 3 of Services.
            self.selected = None;
            self.rows.clear();
            self.visible.clear();
            self.kind = Some(kind.clone());
            self.showing_columns = false;
            self.recolumn(cx);
        }
        self.ask(cx);
        self.rebuild(cx);
    }

    /// Show nothing at all, which is what a change of cluster leaves behind
    /// until discovery lands and a kind is chosen again.
    pub fn show_nothing(&mut self, cx: &mut Context<Self>) {
        self.kind = None;
        self.selected = None;
        self.rows.clear();
        self.visible.clear();
        self.store.update(cx, |store, cx| store.follow(None, cx));
        cx.notify();
    }

    /// Ask for the list on screen, and follow it.
    ///
    /// The two go together: what the table is showing is what is worth
    /// watching, and nothing else is (roadmap §4.7).
    fn ask(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.kind.clone() else {
            return;
        };
        let namespace = self.namespace.clone();
        self.store.update(cx, |store, cx| {
            let followed = store.list_key(&kind, namespace.as_deref());
            store.ensure_list(kind, namespace.as_deref(), cx);
            store.follow(Some(followed), cx);
        });
    }

    /// Scope the table to a namespace, or to every one.
    pub fn set_namespace(&mut self, namespace: Option<String>, cx: &mut Context<Self>) {
        if self.namespace == namespace {
            return;
        }
        self.namespace = namespace;
        self.recolumn(cx);
        self.ask(cx);
        self.rebuild(cx);
    }

    /// Filter the rows.
    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.refilter(cx);
    }

    /// Fetch the list again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(kind) = self.kind.clone() {
            let namespace = self.namespace.clone();
            self.store.update(cx, |store, cx| {
                store.load_list(kind, namespace.as_deref(), cx)
            });
        }
    }

    /// Choose the columns for the kind and the current scope.
    ///
    /// The NAMESPACE column only appears when the table is showing every
    /// namespace, which is the only time it earns its width.
    fn recolumn(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let resource = self.kind.as_ref().and_then(|kind| store.resource(kind));
        let printer = self
            .kind
            .as_ref()
            .and_then(|kind| store.printer_columns(kind));
        let show_namespace = self.namespace.is_none();
        self.default_columns = match resource {
            Some(resource) => ColumnSet::for_resource(resource, show_namespace, printer),
            None => ColumnSet::for_kind("", true, show_namespace),
        };
        self.columns = match self.kind.as_ref() {
            Some(kind) => self.default_columns.clone().with_preferences(
                self.preferences
                    .get(&kind.qualified())
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            ),
            None => self.default_columns.clone(),
        };
        self.sort = self.columns.default_sort();
    }

    fn resolved_preferences(&self) -> Vec<ColumnPreference> {
        let saved = self
            .kind
            .as_ref()
            .and_then(|kind| self.preferences.get(&kind.qualified()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.default_columns.resolved_preferences(saved)
    }

    fn set_preferences(&mut self, preferences: Vec<ColumnPreference>, cx: &mut Context<Self>) {
        let Some(resource) = self.kind.as_ref().map(ResourceKey::qualified) else {
            return;
        };
        self.preferences
            .insert(resource.clone(), preferences.clone());
        self.columns = self.default_columns.clone().with_preferences(&preferences);
        self.sort = self.columns.default_sort();
        // The cells are ordered with the columns, so no row from the prior
        // arrangement can be reused even when its resourceVersion is equal.
        self.rows.clear();
        self.rebuild(cx);
        cx.emit(TableEvent::ColumnsChanged {
            resource,
            preferences,
        });
    }

    fn move_column(&mut self, index: usize, offset: isize, cx: &mut Context<Self>) {
        let mut preferences = self.resolved_preferences();
        let target = index.saturating_add_signed(offset);
        if index >= preferences.len() || target >= preferences.len() || index == target {
            return;
        }
        preferences.swap(index, target);
        self.set_preferences(preferences, cx);
    }

    fn toggle_column(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut preferences = self.resolved_preferences();
        let Some(preference) = preferences.get_mut(index) else {
            return;
        };
        if preference.name == "NAME" {
            return;
        }
        preference.hidden = !preference.hidden;
        self.set_preferences(preferences, cx);
    }

    fn resize_column(&mut self, index: usize, delta: f32, cx: &mut Context<Self>) {
        let mut preferences = self.resolved_preferences();
        let Some(preference) = preferences.get_mut(index) else {
            return;
        };
        let current = preference.width.unwrap_or_else(|| {
            self.default_columns
                .columns()
                .find(|column| column.title == preference.name)
                .map(|column| match column.width {
                    Width::Fixed(width) => width,
                    Width::Flex(flex) => (flex * 80.0).max(80.0),
                })
                .unwrap_or(120.0)
        });
        preference.width = Some((current + delta).clamp(48.0, 600.0));
        self.set_preferences(preferences, cx);
    }

    fn auto_width(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut preferences = self.resolved_preferences();
        let Some(preference) = preferences.get_mut(index) else {
            return;
        };
        preference.width = None;
        self.set_preferences(preferences, cx);
    }

    fn reset_columns(&mut self, cx: &mut Context<Self>) {
        self.set_preferences(Vec::new(), cx);
    }

    /// Rebuild the rows from whatever the store now holds.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.kind.clone() else {
            self.rows.clear();
            self.visible.clear();
            cx.notify();
            return;
        };
        // Discovery — or the CRDs — may have landed since the kind was
        // chosen, in which case the columns were built without a resource,
        // or without the columns its CRD declares, to look at.
        let printer_known = self
            .store
            .read(cx)
            .printer_columns(&kind)
            .is_some_and(|columns| !columns.is_empty());
        if self.columns.kind().is_empty() || (printer_known && self.columns.is_generic()) {
            self.recolumn(cx);
            // A new set of columns is a new set of cells: rows built for the
            // old one must not be reused by their version, or the table would
            // draw three cells under six headings.
            self.rows.clear();
        }
        let now = Utc::now();
        // Taken rather than borrowed, so the rows that survive can be moved
        // into the new set instead of being formatted again: a watch event
        // changes one object, and reformatting four thousand rows for it is
        // what makes a live table expensive (`kirikumo_ui::ColumnSet::rows`).
        let previous = std::mem::take(&mut self.rows);
        let store = self.store.read(cx);
        self.rows = store
            .list(&kind, self.namespace.as_deref())
            .and_then(|list| list.value())
            .map(|list| self.columns.rows(&list.items, &previous, now))
            .unwrap_or_default();
        let (column, ascending) = self.sort;
        table::sort(&mut self.rows, &self.columns, column, ascending);
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.visible = self.filter.indices(&self.query, &self.rows);
        cx.notify();
    }

    fn sort_by(&mut self, column: usize, cx: &mut Context<Self>) {
        // A second click on the same heading reverses it; a first click on
        // another starts that column ascending.
        self.sort = match self.sort {
            (current, ascending) if current == column => (column, !ascending),
            _ => (column, true),
        };
        let (column, ascending) = self.sort;
        table::sort(&mut self.rows, &self.columns, column, ascending);
        self.refilter(cx);
    }

    fn open(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.visible.get(index).and_then(|row| self.rows.get(*row)) else {
            return;
        };
        let Some(kind) = self.kind.clone() else {
            return;
        };
        self.selected = Some(row.key.clone());
        cx.emit(TableEvent::Open((
            kind,
            row.namespace.clone(),
            row.name.clone(),
        )));
        cx.notify();
    }

    /// The sticky heading row.
    fn head(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let (sorted, ascending) = self.sort;
        h_flex()
            .w_full()
            .h(HEAD_HEIGHT)
            .flex_shrink_0()
            .px_2()
            .items_center()
            .bg(tokens.table_head())
            .border_b_1()
            .border_color(tokens.colors().border_subtle)
            // The mark's column has no heading: the mark is the row's state
            // and the STATUS column already says it in words.
            .child(div().w(MARK_WIDTH).flex_shrink_0())
            .children(self.columns.columns().enumerate().map(|(index, column)| {
                let is_sorted = index == sorted;
                h_flex()
                    .id(("head", index))
                    // A flex column takes its share of what the fixed
                    // columns leave; `flex_basis(0)` with `min_w_0` is what
                    // lets a long name truncate instead of pushing the AGE
                    // column off the end.
                    .map(|this| match column.width {
                        Width::Flex(flex) => this.flex_grow(flex).flex_basis(px(0.)).min_w_0(),
                        Width::Fixed(pixels) => this.w(px(pixels)).flex_shrink_0(),
                    })
                    .px_1p5()
                    .gap_1()
                    .items_center()
                    .when(column.numeric, |this| this.justify_end())
                    .cursor_pointer()
                    .text_size(px(10.5))
                    .text_color(match is_sorted {
                        true => tokens.colors().text_secondary,
                        false => tokens.colors().text_muted,
                    })
                    .hover(|this| this.text_color(tokens.colors().text_primary))
                    .on_click(cx.listener(move |this, _, _, cx| this.sort_by(index, cx)))
                    .child(div().truncate().child(column.title.clone()))
                    .when(is_sorted, |this| {
                        this.child(
                            div()
                                .text_size(px(9.))
                                .text_color(tokens.colors().accent)
                                .child(match ascending {
                                    true => "▲",
                                    false => "▼",
                                }),
                        )
                    })
            }))
    }

    /// One row.
    fn row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let Some(row) = self.visible.get(index).and_then(|row| self.rows.get(*row)) else {
            return div().h(ROW_HEIGHT).into_any_element();
        };
        let selected = self.selected.as_deref() == Some(row.key.as_str());
        let mark = tokens.colors().health(row.health.level);

        h_flex()
            .id(("row", index))
            .w_full()
            .h(ROW_HEIGHT)
            .px_2()
            .items_center()
            .cursor_pointer()
            .when(selected, |this| this.bg(tokens.colors().row_active()))
            .hover(|this| this.bg(tokens.colors().row_hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.open(index, cx)))
            .child(
                div().w(MARK_WIDTH).flex_shrink_0().child(
                    Icon::empty()
                        .path(icon::health(row.health.level))
                        .size(px(8.))
                        .text_color(mark),
                ),
            )
            .children(
                self.columns
                    .columns()
                    .zip(row.cells.iter())
                    .map(|(column, cell)| {
                        div()
                            .map(|this| match column.width {
                                Width::Flex(flex) => {
                                    this.flex_grow(flex).flex_basis(px(0.)).min_w_0()
                                }
                                Width::Fixed(pixels) => this.w(px(pixels)).flex_shrink_0(),
                            })
                            .px_1p5()
                            .text_size(px(12.))
                            .when(column.mono, |this| this.font_family("monospace"))
                            .when(column.numeric, |this| this.text_right())
                            .text_color(tokens.colors().text_secondary)
                            .truncate()
                            .child(cell.clone())
                    }),
            )
            .into_any_element()
    }

    /// The small table-local editor for order, visibility and width.
    fn columns_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let preferences = self.resolved_preferences();
        let count = preferences.len();
        let panel_height = (count as f32 * 32.0 + 44.0).min(420.0);
        let control = |id: (&'static str, usize), label: SharedString, enabled: bool| {
            div()
                .id(id)
                .min_w(px(24.))
                .h(px(24.))
                .px_1p5()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(tokens.radius.control()))
                .bg(tokens.colors().bg_surface)
                .text_size(px(10.5))
                .text_color(if enabled {
                    tokens.colors().text_secondary
                } else {
                    tokens.colors().text_muted.opacity(0.45)
                })
                .when(enabled, |this| {
                    this.cursor_pointer()
                        .hover(|this| this.bg(tokens.colors().surface_hover()))
                })
                .child(label)
        };
        let rows =
            preferences
                .into_iter()
                .enumerate()
                .map(|(index, preference)| {
                    let hidden = preference.hidden;
                    let name = preference.name.clone();
                    let width: SharedString = preference
                        .width
                        .map(|width| format!("{width:.0}"))
                        .unwrap_or_else(|| rust_i18n::t!("table.columns.auto").to_string())
                        .into();
                    h_flex()
                        .h(px(32.))
                        .gap_1()
                        .items_center()
                        .child(
                            control(
                                ("col-visible", index),
                                if hidden { "○" } else { "✓" }.into(),
                                name != "NAME",
                            )
                            .when(name != "NAME", |this| {
                                this.on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.toggle_column(index, cx)
                                    }),
                                )
                            }),
                        )
                        .child(
                            div()
                                .w(px(92.))
                                .min_w_0()
                                .text_size(px(11.))
                                .font_family("monospace")
                                .text_color(if hidden {
                                    tokens.colors().text_muted
                                } else {
                                    tokens.colors().text_primary
                                })
                                .truncate()
                                .child(name),
                        )
                        .child(control(("col-up", index), "↑".into(), index > 0).when(
                            index > 0,
                            |this| {
                                this.on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_column(index, -1, cx)
                                }))
                            },
                        ))
                        .child(
                            control(("col-down", index), "↓".into(), index + 1 < count).when(
                                index + 1 < count,
                                |this| {
                                    this.on_click(cx.listener(move |this, _, _, cx| {
                                        this.move_column(index, 1, cx)
                                    }))
                                },
                            ),
                        )
                        .child(control(("col-narrow", index), "−".into(), true).on_click(
                            cx.listener(move |this, _, _, cx| this.resize_column(index, -20.0, cx)),
                        ))
                        .child(control(("col-auto", index), width, true).on_click(
                            cx.listener(move |this, _, _, cx| this.auto_width(index, cx)),
                        ))
                        .child(control(("col-wide", index), "+".into(), true).on_click(
                            cx.listener(move |this, _, _, cx| this.resize_column(index, 20.0, cx)),
                        ))
                });

        v_flex()
            .absolute()
            .top(HEAD_HEIGHT + px(8.))
            .right(px(8.))
            .w(px(330.))
            .h(px(panel_height))
            .p_2()
            .gap_1()
            .occlude()
            .overflow_hidden()
            .bg(tokens.colors().bg_raised)
            .border_1()
            .border_color(tokens.colors().border_strong)
            .rounded(px(tokens.radius.panel))
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(28.))
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_medium()
                            .text_color(tokens.colors().text_primary)
                            .child(rust_i18n::t!("table.columns.title").to_string()),
                    )
                    .child(
                        div()
                            .id("columns-reset")
                            .px_2()
                            .py_1()
                            .rounded(px(tokens.radius.control()))
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .text_color(tokens.colors().text_secondary)
                            .hover(|this| this.bg(tokens.colors().surface_hover()))
                            .on_click(cx.listener(|this, _, _, cx| this.reset_columns(cx)))
                            .child(rust_i18n::t!("table.columns.reset").to_string()),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .children(rows)
                    .overflow_y_scrollbar(),
            )
            .into_any_element()
    }

    /// One muted or red line, for an empty table or a failed one.
    fn notice(&self, text: String, bad: bool, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx);
        div()
            .w_full()
            .px_4()
            .py_3()
            .text_size(px(12.))
            .text_color(match bad {
                true => tokens.colors().status_error,
                false => tokens.colors().text_muted,
            })
            .child(text)
            .into_any_element()
    }
}

impl Render for ResourceTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self
            .kind
            .as_ref()
            .and_then(|kind| self.store.read(cx).list(kind, self.namespace.as_deref()))
            .and_then(|list| list.error())
            .map(str::to_string);
        let loading = self.is_loading(cx);

        let body: AnyElement = if self.kind.is_none() {
            self.notice(rust_i18n::t!("table.pick_a_kind").to_string(), false, cx)
        } else if self.visible.is_empty() {
            match (error, self.rows.is_empty()) {
                // A failure over a table that already had rows keeps them and
                // says why; a failure with nothing on screen is the whole
                // answer.
                (Some(error), true) => self.notice(error, true, cx),
                (_, true) if loading => crate::skeleton::rows(10, ROW_HEIGHT, cx),
                (_, true) => self.notice(rust_i18n::t!("table.empty").to_string(), false, cx),
                (_, false) => self.notice(rust_i18n::t!("table.no_matches").to_string(), false, cx),
            }
        } else {
            let this = cx.entity();
            uniform_list("rows", self.visible.len(), move |range, _window, cx| {
                this.update(cx, |this, cx| {
                    range.map(|index| this.row(index, cx)).collect()
                })
            })
            .flex_1()
            .size_full()
            .into_any_element()
        };

        let head = (self.kind.is_some()).then(|| self.head(cx).into_any_element());

        let columns_panel = self.showing_columns.then(|| self.columns_panel(cx));

        v_flex()
            .size_full()
            .relative()
            .children(head)
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(columns_panel)
    }
}
