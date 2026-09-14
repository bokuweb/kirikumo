//! The standalone window: three resizable columns under three header strips,
//! and no title bar (`docs/ui.md` §3.1).
//!
//! This is the one view a host will not mount: it owns the window's controls,
//! the arrangement and its persistence, all of which are the host's when the
//! views are embedded (`docs/roadmap.md` K1). Everything under it — the
//! sidebar, the table, the detail — is mounted here exactly the way a host
//! would mount it.

use crate::detail::{Detail, DetailEvent};
use crate::palette::{Palette, PaletteEvent};
use crate::sidebar::{Sidebar, SidebarEvent};
use crate::store::Store;
use crate::table::{ResourceTable, TableEvent};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, IconName, InteractiveElementExt as _, StyledExt as _, h_flex, v_flex};
use kirikumo_kube::{Cluster, KubeConfig, ResourceKey, Rest, Scripted};
use kirikumo_ui::assets::icon;
use kirikumo_ui::detail::Target;
use kirikumo_ui::palette::{Action, Command, Here};
use kirikumo_ui::settings::{self, AppSettings};
use kirikumo_ui::{HEADER_HEIGHT, Layout, Mode, Panel, Paths, TRAFFIC_LIGHT_INSET, Tokens, nav};
use std::sync::Arc;
use std::time::{Duration, Instant};

actions!(
    kirikumo,
    [
        ToggleSidebar,
        ToggleRightPanel,
        Refresh,
        FocusFilter,
        PickContext,
        TogglePalette
    ]
);

/// The key context the shell's chords are bound in.
const CONTEXT: &str = "KirikumoShell";

/// How wide a column's grab area is, centred on its edge.
const HANDLE_WIDTH: Pixels = px(9.);

/// The narrowest the centre column is let get.
const CENTRE_MIN: Pixels = px(320.);

/// A divider being dragged.
#[derive(Debug, Clone, Copy)]
struct ColumnDrag {
    panel: Panel,
    start_x: Pixels,
    start_width: Pixels,
}

/// A column on its way open or closed.
#[derive(Debug, Clone, Copy)]
struct Transition {
    began: Instant,
    opening: bool,
}

/// Bind the panel toggles and refresh.
///
/// The chords follow VS Code, because that is the muscle memory everyone
/// using this app already has. Nothing destructive is bound to a key, and
/// nothing ever will be (`AGENTS.md` rule 9).
pub fn init(cx: &mut App) {
    cx.bind_keys([
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-b", ToggleSidebar, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-b", ToggleSidebar, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-alt-b", ToggleRightPanel, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-alt-b", ToggleRightPanel, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-r", Refresh, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-r", Refresh, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-f", FocusFilter, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-f", FocusFilter, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-l", PickContext, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-l", PickContext, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-k", TogglePalette, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-k", TogglePalette, Some(CONTEXT)),
    ]);
}

/// The window.
pub struct Shell {
    paths: Paths,
    settings: AppSettings,
    layout: Layout,
    /// The kubeconfig, for building a connection to another context. `None`
    /// in demo mode, where there is nothing to switch to.
    config: Option<KubeConfig>,
    store: Entity<Store>,
    sidebar: Entity<Sidebar>,
    table: Entity<ResourceTable>,
    detail: Entity<Detail>,
    /// The filter box in the centre strip.
    filter: Entity<InputState>,
    /// The namespace picker's filter box.
    namespace_filter: Entity<InputState>,
    /// Whether the namespace picker is open.
    picking_namespace: bool,
    /// Everything reachable by name. `⌘K`.
    palette: Entity<Palette>,
    /// Whether it is open.
    showing_palette: bool,
    /// The palette was asked for before there was a window to open it in.
    open_palette_pending: bool,
    /// The column panel was asked for before discovery chose a table.
    open_columns_pending: bool,
    /// Brief feedback after the current table has reached the clipboard.
    table_copied: bool,
    /// Something to put in the filter box at the next frame, which is the
    /// next place with a window to put it with.
    pending_filter: Option<String>,
    /// Something to open once discovery has landed.
    open_at_launch: Option<Target>,
    /// Which tab to open it on, by name.
    tab_at_launch: Option<String>,
    /// The appearance changed and the theme has to be installed at the next
    /// frame, which is the first place with a window to ask.
    retheme: bool,
    resizing: Option<ColumnDrag>,
    transitions: Vec<(Panel, Transition)>,
    focus_handle: FocusHandle,
    /// With no title bar the strips are what the window is dragged by, and a
    /// drag is a press that then moved.
    dragging: bool,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// Open over a cluster, with the arrangement the settings remember.
    pub fn new(
        cluster: Arc<dyn Cluster>,
        config: Option<KubeConfig>,
        paths: Paths,
        settings: AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let contexts = config
            .as_ref()
            .map(|config| config.contexts())
            .unwrap_or_default();
        let current = settings
            .last_context
            .clone()
            .filter(|name| contexts.iter().any(|context| &context.name == name))
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.current_context())
                    .map(str::to_string)
            });
        let store = cx.new(|_| Store::new(cluster).with_contexts(contexts, current));

        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rust_i18n::t!("table.filter").to_string())
        });
        let namespace_filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rust_i18n::t!("table.namespace_placeholder").to_string())
        });
        let sidebar = cx.new(|cx| Sidebar::new(store.clone(), window, cx));
        let table =
            cx.new(|cx| ResourceTable::new(store.clone(), settings.table_columns.clone(), cx));
        let detail = cx.new(|cx| Detail::new(store.clone(), window, cx));
        let palette = cx.new(|cx| Palette::new(store.clone(), window, cx));

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&palette, |this, _, event, cx| match event {
            PaletteEvent::Chose(action) => {
                let action = action.clone();
                this.close_palette(cx);
                this.act(action, cx);
            }
            PaletteEvent::Dismissed => this.close_palette(cx),
        }));
        subscriptions.push(cx.subscribe(&sidebar, |this, _, event, cx| match event {
            SidebarEvent::Pick(key) => this.show_kind(key.clone(), cx),
            SidebarEvent::SwitchContext(name) => this.switch_context(name.clone(), cx),
            SidebarEvent::CycleAppearance => this.cycle_appearance(cx),
            SidebarEvent::GroupToggled { id, collapsed } => {
                this.settings.collapsed_groups.retain(|other| other != id);
                if *collapsed {
                    this.settings.collapsed_groups.push(id.clone());
                }
                this.persist();
            }
        }));
        subscriptions.push(cx.subscribe(&detail, |this, _, event, cx| match event {
            DetailEvent::Navigate(target) => this.navigate(target.clone(), cx),
        }));
        subscriptions.push(cx.subscribe(&table, |this, _, event, cx| match event {
            TableEvent::Open(key) => {
                let key = key.clone();
                this.detail.update(cx, |detail, cx| detail.show(key, cx));
                // A row that was opened wants to be read; a closed reading
                // pane would make the click do nothing visible.
                if !this.layout.is_open(Panel::RightPanel) {
                    this.toggle(Panel::RightPanel, cx);
                }
            }
            TableEvent::ColumnsChanged {
                resource,
                preferences,
            } => {
                if preferences.is_empty() {
                    this.settings.table_columns.remove(resource);
                } else {
                    this.settings
                        .table_columns
                        .insert(resource.clone(), preferences.clone());
                }
                this.persist();
            }
        }));
        subscriptions.push(
            cx.subscribe(&filter, |this, filter, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let query = filter.read(cx).value().to_string();
                    this.table
                        .update(cx, |table, cx| table.set_query(query, cx));
                }
            }),
        );
        subscriptions.push(cx.observe(&namespace_filter, |_, _, cx| cx.notify()));
        // The strips show what the store knows (a version, a spinner), so a
        // change there is a redraw here.
        subscriptions.push(
            cx.subscribe(&store, |this, _, _: &crate::store::StoreEvent, cx| {
                // Discovery may have just landed, which is when a kind the
                // settings remembered becomes listable.
                this.adopt_remembered_kind(cx);
                cx.notify();
            }),
        );
        // The OS can change its appearance while the window is open; when the
        // setting is to follow it, the window follows.
        subscriptions.push(window.observe_window_appearance({
            let this = cx.entity().downgrade();
            move |window, cx| {
                this.update(cx, |this, cx| this.apply_theme(window, cx))
                    .ok();
            }
        }));

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        sidebar.update(cx, |sidebar, cx| {
            sidebar.set_appearance(settings.appearance, cx);
            sidebar.set_collapsed(settings.collapsed_groups.iter().cloned(), cx);
        });

        let layout = Layout::from_settings(&settings);
        let namespace = settings.last_namespace.clone();
        let this = Self {
            paths,
            settings,
            layout,
            config,
            store,
            sidebar,
            table,
            detail,
            filter,
            namespace_filter,
            picking_namespace: false,
            palette,
            showing_palette: false,
            open_palette_pending: false,
            open_columns_pending: false,
            table_copied: false,
            pending_filter: None,
            open_at_launch: None,
            tab_at_launch: None,
            retheme: false,
            resizing: None,
            transitions: Vec::new(),
            focus_handle,
            dragging: false,
            _subscriptions: subscriptions,
        };
        this.table
            .update(cx, |table, cx| table.set_namespace(namespace, cx));
        this.store.update(cx, |store, cx| store.refresh_all(cx));
        this
    }

    /// List a kind, and remember that we did.
    fn show_kind(&mut self, key: ResourceKey, cx: &mut Context<Self>) {
        self.settings.last_resource = Some(key.qualified());
        self.persist();
        self.detail.update(cx, |detail, cx| detail.clear(cx));
        self.table.update(cx, |table, cx| table.show(key, cx));
        cx.notify();
    }

    /// Follow a link out of the detail panel.
    ///
    /// Both directions land the same way — the kind is listed, the filter box
    /// is filled in, and the reader can see *why* the table narrowed, because
    /// what narrowed it is written in the box they can clear.
    fn navigate(&mut self, target: Target, cx: &mut Context<Self>) {
        let (key, namespace, query, object) = match target {
            Target::Object {
                key,
                namespace,
                name,
            } => (key, namespace, name.clone(), Some(name)),
            Target::Filtered {
                key,
                namespace,
                query,
            } => (key, namespace, query, None),
        };
        // A cluster-scoped kind is not scoped by the namespace it was reached
        // from: a pod's node does not live in the pod's namespace.
        let namespaced = self
            .store
            .read(cx)
            .resource(&key)
            .is_none_or(|resource| resource.namespaced);
        if namespaced && namespace.is_some() {
            self.set_namespace(namespace.clone(), cx);
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.adopt(Some(key.clone()), cx));
        // `show_kind` clears the panel, so the object is opened after it.
        self.show_kind(key.clone(), cx);
        self.pending_filter = Some(query);
        if let Some(name) = object {
            let namespace = namespace.filter(|_| namespaced);
            self.detail
                .update(cx, |detail, cx| detail.show((key, namespace, name), cx));
        }
        cx.notify();
    }

    /// Point the window at the kind the settings remembered, once discovery
    /// has landed and we know the cluster serves it.
    fn adopt_remembered_kind(&mut self, cx: &mut Context<Self>) {
        if self.table.read(cx).kind().is_some() {
            return;
        }
        let Some(catalogue) = self.store.read(cx).catalogue().value().cloned() else {
            return;
        };
        let wanted = self
            .settings
            .last_resource
            .as_deref()
            .and_then(ResourceKey::parse)
            .filter(|key| catalogue.has(key))
            // A cluster that does not serve what was remembered — or a first
            // run — opens on whatever the sidebar lists first.
            .or_else(|| catalogue.resources.first().map(|resource| resource.key()));
        let Some(key) = wanted else {
            return;
        };
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.adopt(Some(key.clone()), cx));
        self.table.update(cx, |table, cx| table.show(key, cx));
    }

    /// Connect to another context, forgetting the last one entirely.
    fn switch_context(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(config) = &self.config else {
            return;
        };
        let cluster: Arc<dyn Cluster> = match config.access(&name).and_then(Rest::connect) {
            Ok(rest) => Arc::new(rest),
            Err(error) => {
                // The window stays open on an empty cluster and says why in
                // the sidebar, rather than closing or holding the last one's
                // data under the new one's name.
                tracing::error!(%error, context = %name, "could not connect");
                Arc::new(Scripted::empty())
            }
        };
        self.settings.last_context = Some(name.clone());
        self.persist();
        self.detail.update(cx, |detail, cx| detail.clear(cx));
        self.table.update(cx, |table, cx| table.show_nothing(cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.adopt(None, cx));
        self.store
            .update(cx, |store, cx| store.connect(cluster, Some(name), cx));
        cx.notify();
    }

    /// Scope the table to a namespace, or to every one.
    fn set_namespace(&mut self, namespace: Option<String>, cx: &mut Context<Self>) {
        self.picking_namespace = false;
        self.settings.last_namespace = namespace.clone();
        self.persist();
        self.table
            .update(cx, |table, cx| table.set_namespace(namespace, cx));
        cx.notify();
    }

    /// Dark, then light, then the system's, then dark again.
    fn cycle_appearance(&mut self, cx: &mut Context<Self>) {
        self.settings.appearance = self.settings.appearance.next();
        self.persist();
        let appearance = self.settings.appearance;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_appearance(appearance, cx));
        // The theme needs the window's appearance to resolve `System`, and a
        // subscription callback has no window; the next frame does.
        self.retheme = true;
        cx.notify();
    }

    /// Install the theme the setting and the OS agree on, and redraw
    /// everything: the tokens are a global, so every view has to look again.
    fn apply_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = Mode::resolve(self.settings.appearance, window.appearance());
        kirikumo_ui::theme::apply(mode, cx);
        window.refresh();
        cx.notify();
    }

    /// Fetch again what is on screen.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| store.refresh_all(cx));
        self.table.update(cx, |table, cx| table.refresh(cx));
        self.detail.update(cx, |detail, cx| detail.refresh(cx));
    }

    /// Copy exactly the rows and columns currently visible in the centre.
    fn copy_table(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.table.read(cx).clipboard_tsv() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.table_copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1_500))
                .await;
            this.update(cx, |this, cx| {
                this.table_copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.layout.toggle(panel);
        self.persist();
        // The column slides rather than appearing: a panel that pops into
        // place is a layout jump, and one that slides is a thing moving.
        self.transitions.retain(|(other, _)| *other != panel);
        self.transitions.push((
            panel,
            Transition {
                began: Instant::now(),
                opening: self.layout.is_open(panel),
            },
        ));
        cx.notify();
    }

    /// How wide a column is drawn right now: its width, or a fraction of it
    /// while it slides. `None` when it is closed and not sliding.
    fn drawn_width(
        &mut self,
        panel: Panel,
        standard: std::time::Duration,
        window: &mut Window,
    ) -> Option<Pixels> {
        let width = self.layout.size(panel);
        let mut result = self.layout.is_open(panel).then_some(width);
        let mut finished = false;
        if let Some((_, transition)) = self.transitions.iter().find(|(other, _)| *other == panel) {
            let elapsed = transition.began.elapsed().as_secs_f32();
            let t: f32 = (elapsed / standard.as_secs_f32()).min(1.0);
            // Ease out: fast to leave, gentle to land.
            let eased = 1.0 - (1.0 - t).powi(3);
            let fraction = if transition.opening {
                eased
            } else {
                1.0 - eased
            };
            result = Some(width * fraction).filter(|w| *w > px(0.));
            if t >= 1.0 {
                finished = true;
                result = self.layout.is_open(panel).then_some(width);
            } else {
                window.request_animation_frame();
            }
        }
        if finished {
            self.transitions.retain(|(other, _)| *other != panel);
        }
        result
    }

    fn begin_resize(&mut self, panel: Panel, at: Pixels, cx: &mut Context<Self>) {
        self.resizing = Some(ColumnDrag {
            panel,
            start_x: at,
            start_width: self.layout.size(panel),
        });
        cx.notify();
    }

    fn drag_to(&mut self, x: Pixels, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = self.resizing else {
            return;
        };
        let delta = x - drag.start_x;
        // The sidebar's divider is on its right, so the column grows with
        // `x`; the right panel's is on its left, so it shrinks.
        let wanted = match drag.panel {
            Panel::Sidebar => drag.start_width + delta,
            Panel::RightPanel => drag.start_width - delta,
        };
        let (min, max) = match drag.panel {
            Panel::Sidebar => (px(200.), px(480.)),
            Panel::RightPanel => (px(280.), Pixels::MAX),
        };
        let other = match drag.panel {
            Panel::Sidebar => self.drawn_or_zero(Panel::RightPanel),
            Panel::RightPanel => self.drawn_or_zero(Panel::Sidebar),
        };
        let room = window.viewport_size().width - other - CENTRE_MIN;
        let width = wanted.max(min).min(max).min(room.max(min));
        if width != self.layout.size(drag.panel) {
            self.layout.set_size(drag.panel, width);
            cx.notify();
        }
    }

    fn drawn_or_zero(&self, panel: Panel) -> Pixels {
        match self.layout.is_open(panel) {
            true => self.layout.size(panel),
            false => px(0.),
        }
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.persist();
            cx.notify();
        }
    }

    /// The grab area on a column's edge.
    fn handle(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx);
        let active = self.resizing.is_some_and(|drag| drag.panel == panel);
        let line = tokens
            .colors()
            .accent
            .opacity(if active { 0.9 } else { 0.5 });
        let id = match panel {
            Panel::Sidebar => "handle-sidebar",
            Panel::RightPanel => "handle-right",
        };
        div()
            .id(id)
            .absolute()
            .top_0()
            .bottom_0()
            .w(HANDLE_WIDTH)
            .map(|this| match panel {
                Panel::Sidebar => this.right(-HANDLE_WIDTH / 2.),
                Panel::RightPanel => this.left(-HANDLE_WIDTH / 2.),
            })
            .cursor_col_resize()
            .group("handle")
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(HANDLE_WIDTH / 2. - px(0.5))
                    .w(px(1.))
                    .when(active, |this| this.bg(line))
                    .group_hover("handle", |this| this.bg(line)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.begin_resize(panel, event.position.x, cx);
                }),
            )
    }

    fn persist(&mut self) {
        self.layout.write_into(&mut self.settings);
        if let Err(error) = settings::save(&self.paths.app_settings(), &self.settings) {
            tracing::warn!(%error, "could not persist the window's settings");
        }
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(Panel::Sidebar, cx);
    }

    fn on_toggle_right_panel(
        &mut self,
        _: &ToggleRightPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle(Panel::RightPanel, cx);
    }

    fn on_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    /// `⌘F`: put the caret in the filter box.
    ///
    /// It also opens the sidebar's own picker's sibling — no: it only ever
    /// moves focus, because a chord that changed what is listed as well as
    /// where the caret is would surprise.
    fn on_focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.filter.read(cx).focus_handle(cx);
        handle.focus(window, cx);
        cx.notify();
    }

    /// Open one object as soon as the window is up.
    ///
    /// For demos and screenshots (`KIRIKUMO_DEMO_OPEN=Pod/shop/api-…`, the
    /// namespace empty for a cluster-scoped kind). Goes through the same path
    /// a link does, so it exercises what a reader would.
    pub fn open_at_launch(&mut self, target: Target, tab: Option<String>, cx: &mut Context<Self>) {
        self.open_at_launch = Some(target);
        self.tab_at_launch = tab;
        cx.notify();
    }

    /// Open the palette as soon as the window is up.
    ///
    /// For demos and screenshots (`KIRIKUMO_DEMO_PALETTE=1`): a native window
    /// cannot be driven from a script the way a page can, and a screenshot of
    /// the palette is worth an environment variable.
    pub fn open_palette_at_launch(&mut self, cx: &mut Context<Self>) {
        self.open_palette_pending = true;
        cx.notify();
    }

    /// Open the current table's column panel after discovery has landed.
    ///
    /// Used by `KIRIKUMO_DEMO_COLUMNS=1` for deterministic visual checks.
    pub fn open_columns_at_launch(&mut self, cx: &mut Context<Self>) {
        self.open_columns_pending = true;
        cx.notify();
    }

    /// `⌘K`: open the palette, or close it if it is already open.
    fn on_toggle_palette(
        &mut self,
        _: &TogglePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.showing_palette {
            self.close_palette(cx);
            return;
        }
        let here = Here {
            kind: self.table.read(cx).kind().cloned(),
            namespace: self.table.read(cx).namespace().map(str::to_string),
            context: self.store.read(cx).current_context().map(str::to_string),
        };
        self.showing_palette = true;
        self.palette
            .update(cx, |palette, cx| palette.open(here, window, cx));
        cx.notify();
    }

    /// Close the palette and take focus back, so the shell's own chords work
    /// again the moment it is gone.
    fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.showing_palette = false;
        cx.notify();
    }

    /// Do what a palette entry asks.
    ///
    /// Every one of these is something a control on screen already does; the
    /// palette is a second way in, not a second implementation.
    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::Kind(key) => {
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.adopt(Some(key.clone()), cx));
                self.show_kind(key, cx);
            }
            Action::Namespace(namespace) => self.set_namespace(namespace, cx),
            Action::Context(name) => self.switch_context(name, cx),
            Action::Command(Command::Refresh) => self.refresh(cx),
            Action::Command(Command::ToggleSidebar) => self.toggle(Panel::Sidebar, cx),
            Action::Command(Command::ToggleRightPanel) => self.toggle(Panel::RightPanel, cx),
            Action::Command(Command::CycleAppearance) => self.cycle_appearance(cx),
        }
    }

    /// `⌘L`: open the context picker.
    fn on_pick_context(&mut self, _: &PickContext, _: &mut Window, cx: &mut Context<Self>) {
        // The picker lives in the sidebar, so the sidebar has to be showing.
        if !self.layout.is_open(Panel::Sidebar) {
            self.toggle(Panel::Sidebar, cx);
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.toggle_picker(cx));
        cx.notify();
    }

    /// A small square control in a header strip.
    fn icon_button(
        &self,
        id: &'static str,
        icon: Icon,
        tip: String,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let tokens = Tokens::global(cx);
        let hover = tokens.colors().bg_raised;
        let radius = px(tokens.radius.row);
        div()
            .id(id)
            .p_1()
            .rounded(radius)
            .cursor_pointer()
            .hover(move |this| this.bg(hover))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(icon)
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
    }

    /// A control that opens or closes a panel. The icon reports the *state*,
    /// so an open panel shows the "close" variant and reads without hovering.
    fn panel_toggle(
        &self,
        panel: Panel,
        open_icon: IconName,
        closed_icon: IconName,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = self.layout.is_open(panel);
        let color = Tokens::global(cx).colors().text_secondary;
        let id = match panel {
            Panel::Sidebar => "toggle-sidebar",
            Panel::RightPanel => "toggle-right",
        };
        self.icon_button(
            id,
            Icon::new(if open { open_icon } else { closed_icon })
                .size_4()
                .text_color(color),
            rust_i18n::t!(panel.label_key()).to_string(),
            cx,
            move |this, _, cx| this.toggle(panel, cx),
        )
    }

    /// Make a strip the window can be dragged by.
    ///
    /// A drag is a press that then moved: acting on the press alone would
    /// carry the window off whenever a control on the strip was clicked.
    fn draggable(&self, strip: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        strip
            .on_double_click(|_, window, _| window.titlebar_double_click())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.dragging = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.dragging {
                    this.dragging = false;
                    window.start_window_move();
                }
            }))
    }

    /// The window's own controls, across the top of the leading column.
    fn window_controls(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let strip = h_flex()
            .id("window-controls")
            .flex_shrink_0()
            .h(HEADER_HEIGHT)
            .pl(TRAFFIC_LIGHT_INSET)
            .gap_3()
            .items_center()
            .child(self.panel_toggle(
                Panel::Sidebar,
                IconName::PanelLeftClose,
                IconName::PanelLeftOpen,
                cx,
            ));
        self.draggable(strip, cx)
    }

    /// The namespace chip, and the picker it opens.
    fn namespace_chip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let current = self.table.read(cx).namespace().map(str::to_string);
        let label = current
            .clone()
            .unwrap_or_else(|| rust_i18n::t!("table.all_namespaces").to_string());
        // A cluster-scoped kind cannot be scoped to a namespace, so the chip
        // is drawn dimmed and does nothing rather than lying about the
        // table's contents.
        let scoped = self
            .table
            .read(cx)
            .kind()
            .and_then(|kind| self.store.read(cx).resource(kind).cloned())
            .is_none_or(|resource| resource.namespaced);

        h_flex()
            .id("namespace")
            .px_2()
            .py_0p5()
            .gap_1()
            .items_center()
            .flex_shrink_0()
            .rounded(px(tokens.radius.control()))
            .bg(tokens.colors().bg_surface)
            .text_size(px(11.5))
            .text_color(match scoped {
                true => tokens.colors().text_secondary,
                false => tokens.colors().text_muted,
            })
            .when(scoped, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(tokens.colors().surface_hover()))
            })
            .child(div().truncate().child(label))
            .child(
                Icon::empty()
                    .path(icon::CHEVRON_DOWN)
                    .size_3()
                    .text_color(tokens.colors().text_muted),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if scoped {
                    this.picking_namespace = !this.picking_namespace;
                    cx.notify();
                }
            }))
    }

    /// The list of namespaces, when the chip has been clicked.
    fn namespace_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let query = self.namespace_filter.read(cx).value().trim().to_lowercase();
        let current = self.table.read(cx).namespace().map(str::to_string);
        let namespaces: Vec<String> = self
            .store
            .read(cx)
            .namespaces()
            .value()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|name| query.is_empty() || name.to_lowercase().contains(&query))
            .collect();
        let all_selected = current.is_none();

        v_flex()
            .absolute()
            .top(HEADER_HEIGHT)
            .right(px(12.))
            .w(px(260.))
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
                    .child(Input::new(&self.namespace_filter).cleanable(true)),
            )
            .child(
                v_flex()
                    .id("namespaces")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.namespace_row(
                        "all",
                        rust_i18n::t!("table.all_namespaces").to_string(),
                        None,
                        all_selected,
                        cx,
                    ))
                    .children(namespaces.into_iter().enumerate().map(|(index, name)| {
                        let selected = current.as_deref() == Some(name.as_str());
                        self.namespace_row("ns", name.clone(), Some((index, name)), selected, cx)
                    })),
            )
    }

    fn namespace_row(
        &self,
        id: &'static str,
        label: String,
        value: Option<(usize, String)>,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let index = value.as_ref().map(|(index, _)| *index).unwrap_or_default();
        let name = value.map(|(_, name)| name);
        h_flex()
            .id((id, index))
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(px(tokens.radius.row))
            .cursor_pointer()
            .when(selected, |this| this.bg(tokens.colors().row_active()))
            .hover(|this| this.bg(tokens.colors().row_hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.set_namespace(name.clone(), cx)))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(tokens.colors().text_secondary)
                    .truncate()
                    .child(label),
            )
            .when(selected, |this| {
                this.child(
                    Icon::empty()
                        .path(icon::CHECK)
                        .size_3()
                        .text_color(tokens.colors().accent),
                )
            })
            .into_any_element()
    }

    /// The strip across the top of the centre column.
    fn column_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let leading = !self.layout.is_open(Panel::Sidebar);
        let loading = self.table.read(cx).is_loading(cx);
        let title: SharedString = self
            .table
            .read(cx)
            .kind()
            .and_then(|kind| self.store.read(cx).resource(kind).cloned())
            .map(|resource| nav::title(&resource))
            .unwrap_or_else(|| rust_i18n::t!("app.name").to_string())
            .into();
        let (shown, total) = self.table.read(cx).counts();
        let subtitle: SharedString = match total {
            0 => SharedString::default(),
            _ if shown == total => total.to_string().into(),
            _ => rust_i18n::t!("table.rows", shown = shown, total = total)
                .to_string()
                .into(),
        };

        let sidebar_toggle = leading.then(|| {
            self.panel_toggle(
                Panel::Sidebar,
                IconName::PanelLeftClose,
                IconName::PanelLeftOpen,
                cx,
            )
        });
        let namespace = self.namespace_chip(cx).into_any_element();
        let refresh = self.icon_button(
            "refresh",
            Icon::empty()
                .path(icon::REFRESH)
                .size_4()
                .text_color(match loading {
                    true => tokens.colors().accent,
                    false => tokens.colors().text_secondary,
                }),
            rust_i18n::t!("table.refresh").to_string(),
            cx,
            |this, _, cx| this.refresh(cx),
        );
        let columns = self.icon_button(
            "columns",
            Icon::empty()
                .path(icon::SETTINGS)
                .size_4()
                .text_color(tokens.colors().text_secondary),
            rust_i18n::t!("table.columns.open").to_string(),
            cx,
            |this, _, cx| {
                this.table.update(cx, |table, cx| table.toggle_columns(cx));
            },
        );
        let copy = self.icon_button(
            "copy-table",
            match self.table_copied {
                true => Icon::empty()
                    .path(icon::CHECK)
                    .size_4()
                    .text_color(tokens.colors().accent),
                false => Icon::new(IconName::Copy)
                    .size_4()
                    .text_color(tokens.colors().text_secondary),
            },
            rust_i18n::t!(match self.table_copied {
                true => "table.copied",
                false => "table.copy",
            })
            .to_string(),
            cx,
            |this, _, cx| this.copy_table(cx),
        );
        let right_toggle = self.panel_toggle(
            Panel::RightPanel,
            IconName::PanelRightClose,
            IconName::PanelRightOpen,
            cx,
        );

        let strip = h_flex()
            .id("column-header")
            .flex_shrink_0()
            .w_full()
            .h(HEADER_HEIGHT)
            .pr_3()
            .gap_2()
            .items_center()
            .when(leading, |this| this.pl(TRAFFIC_LIGHT_INSET))
            .when(!leading, |this| this.pl_4())
            .children(sidebar_toggle)
            .child(
                h_flex()
                    .flex_1()
                    .gap_2()
                    .items_center()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_medium()
                            .text_color(tokens.colors().text_primary)
                            .truncate()
                            .child(title),
                    )
                    .when(!subtitle.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(tokens.colors().text_muted)
                                .truncate()
                                .child(subtitle),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(namespace)
                    .child(
                        div()
                            .w(px(200.))
                            .flex_shrink_0()
                            .child(Input::new(&self.filter).cleanable(true)),
                    )
                    .child(copy)
                    .child(columns)
                    .child(refresh)
                    .child(right_toggle),
            );
        self.draggable(strip, cx)
    }

    /// The strip across the top of the right column.
    fn right_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let strip = h_flex()
            .id("right-header")
            .flex_shrink_0()
            .w_full()
            .h(HEADER_HEIGHT)
            .px_3()
            .gap_2()
            .items_center()
            .justify_end()
            .border_b_1()
            .border_color(tokens.colors().border_subtle);
        self.draggable(strip, cx)
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.retheme {
            self.retheme = false;
            self.apply_theme(window, cx);
        }
        // Waits for discovery: opened at the first frame the palette would
        // hold four commands and nothing else, which is a screenshot of
        // nothing. A reader pressing ⌘K that early gets the same short list,
        // and the next press gets the full one — it is rebuilt every time.
        if let Some(query) = self.pending_filter.take() {
            self.filter
                .update(cx, |input, cx| input.set_value(query.clone(), window, cx));
            self.table
                .update(cx, |table, cx| table.set_query(query, cx));
        }
        if self.open_at_launch.is_some()
            && self.store.read(cx).catalogue().value().is_some()
            && let Some(target) = self.open_at_launch.take()
        {
            self.navigate(target, cx);
            if let Some(tab) = self.tab_at_launch.take() {
                self.detail
                    .update(cx, |detail, cx| detail.show_tab_named(&tab, cx));
            }
        }
        if self.open_palette_pending && self.store.read(cx).catalogue().value().is_some() {
            self.open_palette_pending = false;
            self.on_toggle_palette(&TogglePalette, window, cx);
        }
        if self.open_columns_pending && self.table.read(cx).kind().is_some() {
            self.open_columns_pending = false;
            self.table.update(cx, |table, cx| table.toggle_columns(cx));
        }
        let tokens = Tokens::global(cx).clone();
        let standard = tokens.duration_ms.standard();
        let sidebar_width = self.drawn_width(Panel::Sidebar, standard, window);
        let right_width = self.drawn_width(Panel::RightPanel, standard, window);
        let sidebar_open = self.layout.is_open(Panel::Sidebar);

        // Built before the column chain: the headers bind listeners, and the
        // chain's own closures hold `self` while they run.
        let window_controls = sidebar_open.then(|| {
            div()
                .w_full()
                .bg(tokens.colors().bg_sidebar)
                .border_r_1()
                .border_color(tokens.colors().border_subtle)
                .child(self.window_controls(cx))
                .into_any_element()
        });
        let column_header = self.column_header(cx).into_any_element();
        let namespace_picker = self
            .picking_namespace
            .then(|| self.namespace_picker(cx).into_any_element());
        let right_header = right_width.map(|_| self.right_header(cx).into_any_element());
        let sidebar_handle =
            sidebar_width.map(|_| self.handle(Panel::Sidebar, cx).into_any_element());
        let right_handle =
            right_width.map(|_| self.handle(Panel::RightPanel, cx).into_any_element());
        let palette = self.showing_palette.then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                // A scrim that catches a click anywhere else: a palette that
                // can only be closed with the keyboard is a trap for whoever
                // opened it with the mouse.
                .child(
                    div()
                        .id("palette-scrim")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .on_click(cx.listener(|this, _, _, cx| this.close_palette(cx))),
                )
                .child(
                    h_flex()
                        .absolute()
                        .top(px(90.))
                        .left_0()
                        .right_0()
                        .justify_center()
                        .child(self.palette.clone()),
                )
                .into_any_element()
        });

        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_toggle_right_panel))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_focus_filter))
            .on_action(cx.listener(Self::on_pick_context))
            .on_action(cx.listener(Self::on_toggle_palette))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_resize(cx)),
            )
            // While a divider is held the pointer is tracked at the window,
            // not on an element: an element only hears moves while it is the
            // one under the pointer, and a drag crosses text fields and
            // scrollbars that claim the pointer for themselves.
            .children(self.resizing.map(|_| {
                let this = cx.entity();
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let on_move = this.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase.bubble() {
                                on_move.update(cx, |this, cx| {
                                    this.drag_to(event.position.x, window, cx)
                                });
                            }
                        });
                        let on_up = this.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase.bubble() {
                                on_up.update(cx, |this, cx| this.end_resize(cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_0()
            }))
            .size_full()
            // The palette hangs off this, so it has to be a positioning
            // context.
            .relative()
            // No background here: `Root` already paints the translucent
            // window and painting it again composites the alpha away.
            .text_color(tokens.colors().text_primary)
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .children(sidebar_width.map(|width| {
                        div()
                            .w(width)
                            .h_full()
                            .flex_shrink_0()
                            .relative()
                            .overflow_hidden()
                            .child(
                                v_flex()
                                    .w(self.layout.size(Panel::Sidebar))
                                    .h_full()
                                    .children(window_controls)
                                    // `flex_1` with a floor of zero: a view
                                    // that took the column's full height under
                                    // a 44 px strip ran past the window and
                                    // took its footer with it.
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .w_full()
                                            .child(self.sidebar.clone()),
                                    ),
                            )
                            .children(sidebar_handle)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w(CENTRE_MIN)
                            .relative()
                            .overflow_hidden()
                            .child(
                                v_flex().size_full().child(column_header).child(
                                    div().flex_1().min_h_0().w_full().child(self.table.clone()),
                                ),
                            )
                            .children(namespace_picker),
                    )
                    .children(right_width.map(|width| {
                        div()
                            .w(width)
                            .h_full()
                            .flex_shrink_0()
                            .relative()
                            .overflow_hidden()
                            .child(
                                v_flex()
                                    .w(self.layout.size(Panel::RightPanel))
                                    .h_full()
                                    .border_l_1()
                                    .border_color(tokens.colors().border_subtle)
                                    .children(right_header)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .w_full()
                                            .child(self.detail.clone()),
                                    ),
                            )
                            .children(right_handle)
                    })),
            )
            .children(palette)
    }
}
