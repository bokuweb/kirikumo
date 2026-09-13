//! The navigation column: `docs/ui.md` §3.2.
//!
//! The cluster at the top, the resource tree under it, the appearance control
//! at the foot. Every row in the tree comes from the catalogue
//! (`kirikumo_ui::nav`), so a cluster that serves a custom resource gets a
//! row for it and nothing here was changed to allow that.

use crate::store::{Store, StoreEvent};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, StyledExt as _, h_flex, v_flex};
use kirikumo_kube::{Group, ResourceKey};
use kirikumo_ui::assets::icon;
use kirikumo_ui::settings::Appearance;
use kirikumo_ui::{Tokens, nav};
use std::collections::HashSet;

/// Emitted when the reader picks something.
pub enum SidebarEvent {
    /// List this kind in the centre column.
    Pick(ResourceKey),
    /// Connect to another context.
    SwitchContext(String),
    /// A group was folded away, or shown again.
    GroupToggled {
        /// Which group, by the name the settings file uses.
        id: String,
        /// Folded, now.
        collapsed: bool,
    },
    /// Move to the next appearance: dark, light, then the system's.
    CycleAppearance,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

/// The navigation column.
pub struct Sidebar {
    store: Entity<Store>,
    selected: Option<ResourceKey>,
    collapsed: HashSet<String>,
    appearance: Appearance,
    /// Whether the context picker is open.
    picking: bool,
    /// Its filter field.
    filter: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl Sidebar {
    /// A sidebar over a store. It redraws whenever the store changes, because
    /// the tree is the catalogue's and the header is the connection's.
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rust_i18n::t!("context.placeholder").to_string())
        });
        let subscriptions = vec![
            cx.subscribe(&store, |_, _, _: &StoreEvent, cx| cx.notify()),
            cx.observe(&filter, |_, _, cx| cx.notify()),
        ];
        Self {
            store,
            selected: None,
            collapsed: HashSet::new(),
            appearance: Appearance::System,
            picking: false,
            filter,
            _subscriptions: subscriptions,
        }
    }

    /// Open or close the context picker. `⌘L`, from the shell.
    pub fn toggle_picker(&mut self, cx: &mut Context<Self>) {
        self.picking = !self.picking;
        cx.notify();
    }

    /// Highlight a kind without emitting, which is how the shell restores a
    /// selection the settings remembered.
    pub fn adopt(&mut self, key: Option<ResourceKey>, cx: &mut Context<Self>) {
        self.selected = key;
        cx.notify();
    }

    /// Tell the footer's control what the window is set to.
    pub fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        self.appearance = appearance;
        cx.notify();
    }

    /// Fold these groups away, as the settings remember.
    pub fn set_collapsed(
        &mut self,
        groups: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        self.collapsed = groups.into_iter().collect();
        cx.notify();
    }

    fn select(&mut self, key: ResourceKey, cx: &mut Context<Self>) {
        self.selected = Some(key.clone());
        cx.emit(SidebarEvent::Pick(key));
        cx.notify();
    }

    fn toggle_group(&mut self, group: Group, cx: &mut Context<Self>) {
        let id = nav::group_id(group).to_string();
        let collapsed = if self.collapsed.remove(&id) {
            false
        } else {
            self.collapsed.insert(id.clone());
            true
        };
        cx.emit(SidebarEvent::GroupToggled { id, collapsed });
        cx.notify();
    }

    fn pick_context(&mut self, name: String, cx: &mut Context<Self>) {
        self.picking = false;
        cx.emit(SidebarEvent::SwitchContext(name));
        cx.notify();
    }

    /// The cluster's name and version, and the way to another one.
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let store = self.store.read(cx);
        let name: SharedString = store
            .current_context()
            .map(str::to_string)
            .unwrap_or_else(|| rust_i18n::t!("sidebar.no_cluster").to_string())
            .into();
        let version: SharedString = store
            .version()
            .value()
            .map(|version| version.label())
            .filter(|label| !label.is_empty())
            .unwrap_or_default()
            .into();
        let insecure = store.is_insecure();
        let server = store.current_server().unwrap_or_default().to_string();

        h_flex()
            .id("cluster")
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .rounded(px(tokens.radius.row))
            .cursor_pointer()
            .hover(|this| this.bg(tokens.colors().row_hover()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.picking = !this.picking;
                cx.notify();
            }))
            .child(
                v_flex()
                    .flex_1()
                    .gap_0p5()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_medium()
                            .text_color(tokens.colors().text_primary)
                            .truncate()
                            .child(name),
                    )
                    .when(!version.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(tokens.colors().text_muted)
                                .truncate()
                                .child(version),
                        )
                    }),
            )
            // An insecure connection is never silent: a viewer that hides
            // that is worse than one that refuses (`docs/ui.md` §3.2).
            .when(insecure, |this| {
                this.child(
                    div()
                        .id("insecure")
                        .tooltip(move |window, cx| {
                            Tooltip::new(
                                rust_i18n::t!("sidebar.insecure", server = server.clone())
                                    .to_string(),
                            )
                            .build(window, cx)
                        })
                        .child(
                            Icon::empty()
                                .path(icon::WARNING)
                                .size_3p5()
                                .text_color(tokens.colors().status_attention),
                        ),
                )
            })
            .child(
                Icon::empty()
                    .path(match self.picking {
                        true => icon::CHEVRON_DOWN,
                        false => icon::CHEVRON_RIGHT,
                    })
                    .size_3()
                    .text_color(tokens.colors().text_muted),
            )
    }

    /// The list of contexts, when the header has been clicked.
    fn context_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let store = self.store.read(cx);
        let current = store.current_context().map(str::to_string);
        let matching: Vec<(String, String, bool)> = store
            .contexts()
            .iter()
            .filter(|context| {
                query.is_empty()
                    || context.name.to_lowercase().contains(&query)
                    || context.server.to_lowercase().contains(&query)
            })
            .map(|context| {
                (
                    context.name.clone(),
                    context.server.clone(),
                    Some(&context.name) == current.as_ref(),
                )
            })
            .collect();

        v_flex()
            .absolute()
            .top(px(52.))
            .left(px(8.))
            .right(px(8.))
            .max_h(px(360.))
            .p_1()
            .gap_0p5()
            .rounded(px(tokens.radius.panel))
            .bg(tokens.colors().bg_raised)
            .border_1()
            .border_color(tokens.colors().border_subtle)
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .child(Input::new(&self.filter).cleanable(true)),
            )
            .child(
                v_flex()
                    .id("contexts")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(matching.is_empty(), |this| {
                        this.child(
                            div()
                                .px_2()
                                .py_1()
                                .text_size(px(11.5))
                                .text_color(tokens.colors().text_muted)
                                .child(rust_i18n::t!("context.empty").to_string()),
                        )
                    })
                    .children(matching.into_iter().enumerate().map(
                        |(index, (name, server, selected))| {
                            let picked = name.clone();
                            h_flex()
                                .id(("context", index))
                                .w_full()
                                .px_2()
                                .py_1()
                                .gap_2()
                                .items_center()
                                .rounded(px(tokens.radius.row))
                                .cursor_pointer()
                                .when(selected, |this| this.bg(tokens.colors().row_active()))
                                .hover(|this| this.bg(tokens.colors().row_hover()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pick_context(picked.clone(), cx)
                                }))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .text_color(tokens.colors().text_primary)
                                                .truncate()
                                                .child(name),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(tokens.colors().text_muted)
                                                .truncate()
                                                .child(server),
                                        ),
                                )
                                .when(selected, |this| {
                                    this.child(
                                        Icon::empty()
                                            .path(icon::CHECK)
                                            .size_3()
                                            .text_color(tokens.colors().accent),
                                    )
                                })
                        },
                    )),
            )
    }

    /// A group's heading, which folds it.
    fn group_row(&self, group: Group, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let collapsed = self.collapsed.contains(nav::group_id(group));
        h_flex()
            .id(SharedString::from(format!(
                "group:{}",
                nav::group_id(group)
            )))
            .w_full()
            .px_2p5()
            .py_1()
            .mt_2()
            .gap_1p5()
            .items_center()
            .rounded(px(tokens.radius.row))
            .cursor_pointer()
            .hover(|this| this.bg(tokens.colors().row_hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(group, cx)))
            .child(
                Icon::empty()
                    .path(match collapsed {
                        true => icon::CHEVRON_RIGHT,
                        false => icon::CHEVRON_DOWN,
                    })
                    .size_3()
                    .text_color(tokens.colors().text_muted),
            )
            .child(
                Icon::empty()
                    .path(nav::group_icon(group))
                    .size_3p5()
                    .text_color(tokens.colors().text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.5))
                    .text_color(tokens.colors().text_muted)
                    .truncate()
                    .child(rust_i18n::t!(group.label_key()).to_string()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(tokens.colors().text_muted)
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    /// One kind.
    fn kind_row(&self, index: usize, row: &nav::NavRow, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let selected = self.selected.as_ref() == Some(&row.key);
        let key = row.key.clone();
        // The number of rows in the list, once it has landed, and how many of
        // them are worth looking at.
        let counts = self
            .store
            .read(cx)
            .list(&row.key, None)
            .and_then(|list| list.value())
            .map(|list| list.items.len());
        h_flex()
            .id(("kind", index))
            .w_full()
            .pl(px(30.))
            .pr_2p5()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(px(tokens.radius.row))
            .cursor_pointer()
            .when(selected, |this| this.bg(tokens.colors().row_active()))
            .hover(|this| this.bg(tokens.colors().row_hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.select(key.clone(), cx)))
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .when(selected, |this| this.font_medium())
                    .text_color(match selected {
                        true => tokens.colors().text_primary,
                        false => tokens.colors().text_secondary,
                    })
                    .truncate()
                    .child(row.title.clone()),
            )
            .children(counts.map(|count| {
                div()
                    .text_size(px(11.))
                    .text_color(tokens.colors().text_muted)
                    .child(count.to_string())
            }))
            .into_any_element()
    }

    /// An API group's heading, inside Custom Resources.
    fn subheading(&self, heading: &str, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx);
        div()
            .w_full()
            .pl(px(30.))
            .pr_2p5()
            .pt_1p5()
            .pb_0p5()
            .text_size(px(10.5))
            .text_color(tokens.colors().text_muted)
            .truncate()
            .child(heading.to_string())
            .into_any_element()
    }

    /// The appearance control: a moon, a sun, or half of each.
    fn appearance_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx);
        let path = match self.appearance {
            Appearance::Dark => icon::MOON,
            Appearance::Light => icon::SUN,
            Appearance::System => icon::SUN_MOON,
        };
        div()
            .id("appearance")
            .p_1()
            .rounded(px(tokens.radius.row))
            .cursor_pointer()
            .hover(|this| this.bg(tokens.colors().row_hover()))
            .tooltip(|window, cx| {
                Tooltip::new(rust_i18n::t!("sidebar.appearance").to_string()).build(window, cx)
            })
            .child(
                Icon::empty()
                    .path(path)
                    .size_3p5()
                    .text_color(tokens.colors().text_muted),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::CycleAppearance)))
    }

    /// The strip at the foot: what is loaded, and the appearance control.
    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let appearance = self.appearance_button(cx).into_any_element();
        let tokens = Tokens::global(cx);
        let store = self.store.read(cx);
        let held: usize = store
            .catalogue()
            .value()
            .map(|catalogue| catalogue.resources.len())
            .unwrap_or_default();
        // What the connection is doing, in one line: what went wrong, or that
        // the list on screen is being followed, or how much the cluster
        // serves (`docs/ui.md` §3.2).
        let summary: SharedString = match (store.catalogue().error(), store.is_live()) {
            (Some(error), _) => error.to_string().into(),
            (None, true) => rust_i18n::t!("sidebar.watching").to_string().into(),
            (None, false) => rust_i18n::t!("sidebar.kinds", count = held)
                .to_string()
                .into(),
        };
        let failed = store.catalogue().error().is_some();
        let live = store.is_live();

        h_flex()
            .w_full()
            .px_3()
            .py_2p5()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(tokens.colors().border_subtle)
            .when(live, |this| {
                this.child(
                    Icon::empty()
                        .path(icon::HEALTH_OK)
                        .size(px(6.))
                        .text_color(tokens.colors().status_done),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.5))
                    .text_color(match failed {
                        true => tokens.colors().status_error,
                        false => tokens.colors().text_muted,
                    })
                    .truncate()
                    .child(summary),
            )
            .child(appearance)
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = Tokens::global(cx).clone();
        let sections = self
            .store
            .read(cx)
            .catalogue()
            .value()
            .map(nav::sections)
            .unwrap_or_default();
        let loading = self.store.read(cx).catalogue().is_loading();

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut index = 0;
        for section in &sections {
            rows.push(self.group_row(section.group, section.len(), cx));
            if self.collapsed.contains(nav::group_id(section.group)) {
                index += section.len();
                continue;
            }
            for subsection in &section.subsections {
                if let Some(heading) = &subsection.heading {
                    rows.push(self.subheading(heading, cx));
                }
                for row in &subsection.rows {
                    rows.push(self.kind_row(index, row, cx));
                    index += 1;
                }
            }
        }

        let header = self.header(cx).into_any_element();
        let picker = self
            .picking
            .then(|| self.context_picker(cx).into_any_element());
        let footer = self.footer(cx).into_any_element();

        v_flex()
            .size_full()
            .relative()
            .bg(tokens.colors().bg_sidebar)
            .border_r_1()
            .border_color(tokens.colors().border_subtle)
            .child(div().px_2().pt_1().child(header))
            .child(
                v_flex()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(sections.is_empty(), |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_size(px(11.5))
                                .text_color(tokens.colors().text_muted)
                                .child(match loading {
                                    true => rust_i18n::t!("sidebar.loading_catalogue").to_string(),
                                    false => rust_i18n::t!("sidebar.no_cluster").to_string(),
                                }),
                        )
                    }),
            )
            .child(footer)
            .children(picker)
    }
}
