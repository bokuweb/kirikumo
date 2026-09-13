//! The right panel: `docs/ui.md` §3.4.
//!
//! Tabs over one object — Overview, Events, YAML, an Argo CD Application's
//! Resources, a Pod's, selecting workload's or Node's Logs, and a Pod's Run
//! and Shell. What each of them says is decided in `kirikumo_ui`
//! (`detail::overview`, `terminal::Screen`) or in
//! `kirikumo_kube` (`yaml::to_yaml`); this file draws it.
//!
//! The object itself is read out of the list the table is already showing
//! rather than fetched again: it arrived a moment ago, and a `GET` for it
//! would put a spinner over data the window already has.

use crate::store::{ObjectKey, ShellStatus, Store, StoreEvent, Write};
use chrono::Utc;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, StyledExt as _, h_flex, v_flex};
use kirikumo_kube::{Action, ExecRequest, LogRequest, Object, actions, yaml};
use kirikumo_ui::actions::{Pending, confirm_label};
use kirikumo_ui::assets::icon;
use kirikumo_ui::detail::Target;
use kirikumo_ui::terminal::{self, Style};
use kirikumo_ui::{Tokens, detail, logs, time};
use serde_json::Value;

/// How tall one line of YAML or of a log is.
const LINE_HEIGHT: Pixels = px(17.);

/// A log control row stays outside the virtualized log's paint bounds.
const LOG_TOOLBAR_HEIGHT: Pixels = px(38.);

/// The size the shell's text is drawn at.
const SHELL_FONT_SIZE: Pixels = px(12.5);

/// The size a shell is told before its panel has been measured.
const SHELL_DEFAULT: (u16, u16) = (80, 24);

/// Which tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The facts.
    Overview,
    /// Resources managed by an Argo CD Application.
    Resources,
    /// What has happened to it.
    Events,
    /// The object as the apiserver holds it.
    Yaml,
    /// A container's output.
    Logs,
    /// A command, run in a container.
    Run,
    /// A shell attached to a container.
    Shell,
}

impl Tab {
    /// The locale key for the tab's name.
    fn label_key(self) -> &'static str {
        match self {
            Self::Overview => "detail.overview",
            Self::Resources => "detail.resources",
            Self::Events => "detail.events",
            Self::Yaml => "detail.yaml",
            Self::Logs => "detail.logs",
            Self::Run => "detail.run",
            Self::Shell => "detail.shell",
        }
    }
}

/// What the reader did in the panel.
pub enum DetailEvent {
    /// Go to something this object points at: up to its controller, or down
    /// to what its selector selects.
    Navigate(Target),
}

impl EventEmitter<DetailEvent> for Detail {}

/// The right panel.
pub struct Detail {
    store: Entity<Store>,
    key: Option<ObjectKey>,
    tab: Tab,
    /// Which Pod supplies a workload's or Node's log. A Pod detail needs no choice.
    pod: Option<(Option<String>, String)>,
    /// Which container's log is showing, when the object has more than one.
    container: Option<String>,
    /// Whether the log is being followed as the container writes.
    following: bool,
    /// Whether to read the *previous* instance's log, which is the only place
    /// a crash loop's reason survives.
    previous: bool,
    /// The find box over the log.
    find: Entity<InputState>,
    scroll: UniformListScrollHandle,
    /// How many lines the log had at the last frame, so that following can
    /// tell "something arrived" from "nothing did".
    last_lines: usize,
    /// Whether the last frame scrolled the log itself, which is the one case
    /// where "not at the end" does not mean the reader moved it.
    scrolled_last_frame: bool,
    /// A write between its first gesture and its second.
    pending: Pending,
    /// The replica count field, for a scale.
    replicas: Entity<InputState>,
    /// The YAML tab: the toolkit's editor, read-only until *Edit*.
    editor: Entity<EditorState>,
    /// Whether the YAML is being edited, which is when the editor's text is
    /// the reader's and must not be replaced by the object's.
    editing: bool,
    /// Which object and version the editor was last filled from, so it is
    /// refilled when either changes and left alone otherwise.
    editor_holds: Option<(ObjectKey, String)>,
    /// Why an edited manifest was refused before it was sent, if it was.
    apply_error: Option<String>,
    /// Why the last forward could not be started, if it could not.
    forward_error: Option<String>,
    /// The command field on the Run tab.
    command: Entity<InputState>,
    /// Keyboard focus for the Shell tab: what is typed while it has focus
    /// goes to the shell, and nowhere else.
    shell_focus: FocusHandle,
    /// The shell panel's size in cells at the last frame, so a resize is
    /// noticed and a frame that changed nothing costs nothing.
    shell_cells: Option<(u16, u16)>,
    /// A shell asked for before the cluster has said whether it may be
    /// opened; attached the moment it says yes. For demos, which open the
    /// tab and expect a prompt in it.
    attach_when_allowed: bool,
}

impl Detail {
    /// A panel over a store, showing nothing until told what to.
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let find = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rust_i18n::t!("detail.find").to_string())
        });
        cx.subscribe(&find, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        })
        .detach();
        let replicas = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rust_i18n::t!("action.replicas").to_string())
        });
        cx.subscribe(&replicas, |this, replicas, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Pending::Armed {
                    action: Action::Scale,
                    replicas: typed,
                } = &mut this.pending
            {
                *typed = replicas.read(cx).value().to_string();
                cx.notify();
            }
        })
        .detach();
        let editor = cx.new(|cx| EditorState::new(window, cx).language("yaml"));
        let command = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rust_i18n::t!("detail.run_placeholder").to_string())
        });
        cx.subscribe(&command, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.run_command(cx);
            }
        })
        .detach();
        // Asked again on every change, not just when a tab is opened: an
        // object reached by a link is drawn before the list it lives in has
        // landed, so the moment it does the tab has to ask for its events.
        // `ensure` is idempotent — it starts a fetch only when one is idle.
        cx.subscribe(&store, |this: &mut Self, _, _: &StoreEvent, cx| {
            this.ensure(cx);
            cx.notify();
        })
        .detach();
        Self {
            store,
            key: None,
            tab: Tab::Overview,
            pod: None,
            container: None,
            // Following is what a person opening a log wants; a tail that
            // stops the moment it is drawn is a screenshot.
            following: true,
            previous: false,
            find,
            scroll: UniformListScrollHandle::new(),
            last_lines: 0,
            scrolled_last_frame: false,
            pending: Pending::Idle,
            replicas,
            editor,
            editing: false,
            editor_holds: None,
            apply_error: None,
            forward_error: None,
            command,
            shell_focus: cx.focus_handle(),
            shell_cells: None,
            attach_when_allowed: false,
        }
    }

    /// Show an object.
    pub fn show(&mut self, key: ObjectKey, cx: &mut Context<Self>) {
        if self.key.as_ref() != Some(&key) {
            // A different object: the tab stays, because a reader stepping
            // down a list of pods with the YAML tab open wants the next
            // pod's YAML — but the container does not, since it named one
            // of the last object's, and neither does a shell into it.
            self.pod = None;
            self.container = None;
            self.store.update(cx, |store, _| store.detach_shell());
        }
        if self.tab == Tab::Resources && !is_argo_application(&key.0) {
            self.tab = Tab::Overview;
        }
        if self.tab == Tab::Events && !detail::has_related_events(&key.0) {
            self.tab = Tab::Overview;
        }
        self.key = Some(key);
        // A write armed on one object must not fire on the next.
        self.pending = Pending::Idle;
        self.editing = false;
        self.apply_error = None;
        self.forward_error = None;
        self.stop_following(cx);
        self.ensure(cx);
        cx.notify();
    }

    /// Nothing is open.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.key = None;
        self.stop_following(cx);
        self.store.update(cx, |store, _| store.detach_shell());
        cx.notify();
    }

    /// Stop reading a log, which every move away from one has to do: a
    /// followed log is a thread and a connection, and nothing else on screen
    /// needs either.
    fn stop_following(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |store, _| store.stop_following_log());
        self.last_lines = 0;
    }

    /// Fetch again whatever the open tab is showing.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.tab == Tab::Logs {
            self.refresh_log_pods(cx);
            self.reload_log(cx);
            return;
        }
        if self.tab == Tab::Events {
            if let Some(object) = self.object(cx) {
                self.store.update(cx, |store, cx| {
                    store.load_events(object.meta.uid, object.meta.namespace, cx)
                });
            }
            return;
        }
        self.ensure(cx);
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == Tab::Logs && tab != Tab::Logs {
            self.stop_following(cx);
        }
        let entering_logs = tab == Tab::Logs && self.tab != Tab::Logs;
        self.tab = tab;
        // Coming *back* to a log has to start following again, and `ensure`
        // will not: it asks only for what has never been asked for, and this
        // log's lines are still here from last time.
        match entering_logs {
            true => {
                self.ensure(cx);
                self.reload_log(cx);
            }
            false => self.ensure(cx),
        }
        cx.notify();
    }

    /// Open a tab by name. For demos and screenshots
    /// (`KIRIKUMO_DEMO_OPEN=…#yaml`); an unknown name is the Overview.
    pub fn show_tab_named(&mut self, name: &str, cx: &mut Context<Self>) {
        let mut tab = match name {
            "events" => Tab::Events,
            "resources" => Tab::Resources,
            "yaml" => Tab::Yaml,
            "logs" => Tab::Logs,
            "run" => Tab::Run,
            "shell" => Tab::Shell,
            _ => Tab::Overview,
        };
        if tab == Tab::Resources
            && !self
                .key
                .as_ref()
                .is_some_and(|(resource, _, _)| is_argo_application(resource))
        {
            tab = Tab::Overview;
        }
        if tab == Tab::Events
            && !self
                .key
                .as_ref()
                .is_some_and(|(resource, _, _)| detail::has_related_events(resource))
        {
            tab = Tab::Overview;
        }
        if let Some(object) = self.object(cx)
            && !self.tab_available(tab, &object)
        {
            tab = Tab::Overview;
        }
        self.attach_when_allowed = tab == Tab::Shell;
        self.set_tab(tab, cx);
    }

    /// Whether a tab has a real API target for the object now in the panel.
    fn tab_available(&self, tab: Tab, object: &Object) -> bool {
        let Some((resource, _, _)) = self.key.as_ref() else {
            return false;
        };
        match tab {
            Tab::Overview | Tab::Yaml => true,
            Tab::Resources => is_argo_application(resource),
            Tab::Events => detail::has_related_events(resource),
            Tab::Logs => detail::has_log_view(resource, object),
            Tab::Run | Tab::Shell => detail::has_exec_view(resource, object),
        }
    }

    /// Ask for the log again under whatever the toggles now say.
    fn reload_log(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.log_request(cx) else {
            return;
        };
        let following = self.following;
        self.last_lines = 0;
        self.store
            .update(cx, |store, cx| store.show_log(request, following, cx));
        cx.notify();
    }

    /// Ask for whatever the open tab needs and does not have.
    fn ensure(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        if !self.tab_available(self.tab, &object) {
            let stopped_log = self.tab == Tab::Logs;
            self.tab = Tab::Overview;
            self.attach_when_allowed = false;
            if stopped_log {
                self.stop_following(cx);
            }
        }
        // Usage is on the Overview, which is the tab this panel opens on, and
        // it is one request per namespace rather than per object.
        let kind = self.kind();
        let namespace = object.meta.namespace.clone();
        self.store.update(cx, |store, cx| {
            store.ensure_metrics(&kind, namespace.as_deref(), cx)
        });
        // And whether this login may do each thing the footer offers, so a
        // button is lit or grey before it is pressed rather than after.
        if let Some((key, resource)) =
            self.key
                .as_ref()
                .map(|(key, _, _)| key.clone())
                .and_then(|key| {
                    let resource = self.store.read(cx).resource(&key).cloned()?;
                    Some((key, resource))
                })
        {
            let verbs: Vec<&'static str> = actions::available(&resource, &object)
                .into_iter()
                .map(Action::verb)
                .collect();
            let namespace = namespace.filter(|_| resource.namespaced);
            self.store.update(cx, |store, cx| {
                for verb in verbs {
                    store.ensure_permission(key.clone(), namespace.clone(), verb, cx);
                }
            });
        }
        match self.tab {
            Tab::Events => {
                let uid = object.meta.uid.clone();
                let namespace = object.meta.namespace.clone();
                self.store
                    .update(cx, |store, cx| store.ensure_events(uid, namespace, cx));
            }
            Tab::Logs => {
                if let Some(namespace) = self.indirect_log_namespace(&object) {
                    self.store.update(cx, |store, cx| {
                        store.ensure_list(
                            kirikumo_kube::ResourceKey::new("", "Pod"),
                            namespace.as_deref(),
                            cx,
                        )
                    });
                }
                if let Some(request) = self.log_request(cx) {
                    let following = self.following;
                    // Only when nothing has been asked for yet: `ensure` runs
                    // on every store change, and restarting a followed log on
                    // each of its own lines would be a loop.
                    let asked = self
                        .store
                        .read(cx)
                        .logs(&Store::log_key(&request))
                        .is_some_and(|fetch| !fetch.is_idle());
                    if !asked {
                        self.store
                            .update(cx, |store, cx| store.show_log(request, following, cx));
                    }
                }
            }
            Tab::Run | Tab::Shell => {
                let namespace = object.meta.namespace.clone();
                let allowed = self.store.read(cx).exec_permission(namespace.as_deref());
                self.store
                    .update(cx, |store, cx| store.ensure_exec_permission(namespace, cx));
                if self.tab == Tab::Shell && self.attach_when_allowed && allowed.is_some() {
                    self.attach_when_allowed = false;
                    self.attach_shell(cx);
                }
            }
            Tab::Overview | Tab::Resources | Tab::Yaml => {}
        }
    }

    /// The kind being shown.
    fn kind(&self) -> String {
        self.key
            .as_ref()
            .map(|(kind, _, _)| kind.kind.clone())
            .unwrap_or_default()
    }

    /// The object being shown, out of the store's lists.
    fn object(&self, cx: &App) -> Option<Object> {
        let key = self.key.as_ref()?;
        self.store.read(cx).object(key).cloned()
    }

    /// The containers this object has, if it is the kind that has any.
    fn containers(&self, cx: &App) -> Vec<String> {
        self.object(cx)
            .map(|object| {
                object
                    .array_at("spec.containers")
                    .iter()
                    .filter_map(|container| container.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether this panel resolves a workload or Node to an explicit Pod.
    fn indirect_log_namespace(&self, object: &Object) -> Option<Option<String>> {
        if detail::has_workload_logs(object) {
            return Some(object.meta.namespace.clone());
        }
        self.key
            .as_ref()
            .is_some_and(|(resource, _, _)| is_node(resource))
            .then_some(None)
    }

    /// Pods selected by the workload or Node in this panel, newest first.
    fn log_pods(&self, cx: &App) -> Vec<Object> {
        let Some(subject) = self.object(cx) else {
            return Vec::new();
        };
        let Some(namespace) = self.indirect_log_namespace(&subject) else {
            return Vec::new();
        };
        let pod_key = kirikumo_kube::ResourceKey::new("", "Pod");
        let Some(pods) = self
            .store
            .read(cx)
            .list(&pod_key, namespace.as_deref())
            .and_then(|fetch| fetch.value())
        else {
            return Vec::new();
        };
        let selected = match self
            .key
            .as_ref()
            .is_some_and(|(resource, _, _)| is_node(resource))
        {
            true => detail::node_log_pods(&subject, &pods.items),
            false => detail::workload_log_pods(&subject, &pods.items),
        };
        selected.into_iter().cloned().collect()
    }

    /// The Pod whose log is currently selected.
    fn log_pod(&self, cx: &App) -> Option<Object> {
        let object = self.object(cx)?;
        if self.indirect_log_namespace(&object).is_none() {
            return Some(object);
        }
        let pods = self.log_pods(cx);
        self.pod
            .as_ref()
            .and_then(|(namespace, name)| {
                pods.iter()
                    .find(|pod| &pod.meta.namespace == namespace && &pod.meta.name == name)
            })
            .or_else(|| pods.first())
            .cloned()
    }

    /// Containers on the Pod currently supplying the log.
    fn log_containers(&self, cx: &App) -> Vec<String> {
        self.log_pod(cx)
            .map(|pod| {
                pod.array_at("spec.containers")
                    .iter()
                    .filter_map(|container| container.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn refresh_log_pods(&mut self, cx: &mut Context<Self>) {
        let Some(subject) = self.object(cx) else {
            return;
        };
        let Some(namespace) = self.indirect_log_namespace(&subject) else {
            return;
        };
        self.store.update(cx, |store, cx| {
            store.load_list(
                kirikumo_kube::ResourceKey::new("", "Pod"),
                namespace.as_deref(),
                cx,
            )
        });
    }

    /// What to ask for on the Logs tab.
    fn log_request(&self, cx: &App) -> Option<LogRequest> {
        let pod = self.log_pod(cx)?;
        let namespace = pod.meta.namespace.clone()?;
        let containers = self.log_containers(cx);
        let container = self
            .container
            .clone()
            .filter(|selected| containers.contains(selected))
            .or_else(|| containers.first().cloned())?;
        Some(
            LogRequest::new(namespace, pod.meta.name.clone())
                .container(container)
                .previous(self.previous),
        )
    }

    /// The name, the kind and the health, across the top.
    fn header(&self, object: &Object, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let kind = self.kind();
        let health = match self.key.as_ref() {
            Some((resource, _, _)) => kirikumo_kube::health::of_resource(resource, object),
            None => kirikumo_kube::health::of(&kind, object),
        };
        let mut where_it_is = kind.clone();
        if let Some(namespace) = &object.meta.namespace {
            where_it_is.push_str(" · ");
            where_it_is.push_str(namespace);
        }

        v_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap_1p5()
            .border_b_1()
            .border_color(tokens.colors().border_subtle)
            .child(
                div()
                    .text_size(px(15.))
                    .font_medium()
                    .font_family("monospace")
                    .text_color(tokens.colors().text_primary)
                    .child(object.meta.name.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(tokens.colors().text_muted)
                            .child(where_it_is),
                    )
                    .when(!health.word.is_empty(), |this| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(
                                    Icon::empty()
                                        .path(icon::health(health.level))
                                        .size(px(8.))
                                        .text_color(tokens.colors().health(health.level)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(tokens.colors().text_secondary)
                                        .child(health.word.clone()),
                                ),
                        )
                    }),
            )
    }

    /// The tab chips.
    fn tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tokens = Tokens::global(cx).clone();
        let object = self.object(cx);
        let has_logs = object
            .as_ref()
            .is_some_and(|object| self.tab_available(Tab::Logs, object));
        let has_exec = object
            .as_ref()
            .is_some_and(|object| self.tab_available(Tab::Run, object));
        let mut tabs = vec![Tab::Overview];
        if self
            .key
            .as_ref()
            .is_some_and(|(resource, _, _)| is_argo_application(resource))
        {
            tabs.push(Tab::Resources);
        }
        if self
            .key
            .as_ref()
            .is_some_and(|(resource, _, _)| detail::has_related_events(resource))
        {
            tabs.push(Tab::Events);
        }
        tabs.push(Tab::Yaml);
        if has_logs {
            tabs.push(Tab::Logs);
        }
        if has_exec {
            tabs.extend([Tab::Run, Tab::Shell]);
        }
        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .gap_1()
            .children(tabs.into_iter().enumerate().map(|(index, tab)| {
                let selected = tab == self.tab;
                div()
                    .id(("tab", index))
                    .px_2p5()
                    .py_1()
                    .rounded(px(tokens.radius.control()))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .when(selected, |this| {
                        this.bg(tokens.colors().row_active())
                            .text_color(tokens.colors().text_primary)
                    })
                    .when(!selected, |this| {
                        this.text_color(tokens.colors().text_muted)
                    })
                    .hover(|this| this.bg(tokens.colors().row_hover()))
                    .child(rust_i18n::t!(tab.label_key()).to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            }))
    }

    /// The Overview tab.
    fn overview(&self, object: &Object, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let kind = self.kind();
        let usage = self
            .store
            .read(cx)
            .metrics_for(&kind, object.meta.namespace.as_deref(), &object.meta.name)
            .cloned();
        let overview = match self.key.as_ref() {
            Some((resource, _, _)) => {
                detail::overview_for(resource, object, usage.as_ref(), Utc::now())
            }
            None => detail::overview(&kind, object, usage.as_ref(), Utc::now()),
        };

        v_flex()
            .id("overview")
            .size_full()
            .px_4()
            .py_3()
            .gap_4()
            .overflow_y_scroll()
            .children(overview.sections.into_iter().map(|section| {
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .children(section.title.map(|title| {
                        div()
                            .text_size(px(10.5))
                            .text_color(tokens.colors().text_muted)
                            .child(title)
                    }))
                    .children(section.facts.into_iter().enumerate().map(|(index, fact)| {
                        let link = fact.link.clone();
                        h_flex()
                            .w_full()
                            .gap_3()
                            .items_start()
                            .child(
                                div()
                                    .w(px(104.))
                                    .flex_shrink_0()
                                    .text_size(px(11.5))
                                    .text_color(tokens.colors().text_muted)
                                    .child(fact.label),
                            )
                            .child(
                                div()
                                    .id(("fact", index))
                                    .flex_1()
                                    .text_size(px(12.))
                                    .when(fact.mono, |this| this.font_family("monospace"))
                                    // A fact that goes somewhere is drawn as a
                                    // link, in the accent, and never as an
                                    // underline in the muted grey that reads
                                    // as struck out.
                                    .when(link.is_some(), |this| {
                                        this.cursor_pointer()
                                            .text_color(tokens.colors().accent)
                                            .hover(|this| {
                                                this.text_color(tokens.colors().accent.opacity(0.8))
                                            })
                                    })
                                    .when(link.is_none(), |this| {
                                        this.text_color(tokens.colors().text_secondary)
                                    })
                                    .child(fact.value)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        if let Some(target) = link.clone() {
                                            cx.emit(DetailEvent::Navigate(target));
                                        }
                                    })),
                            )
                    }))
            }))
            .when(kind == "Pod", |this| this.child(self.forwards(object, cx)))
            .when(!overview.conditions.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .gap_1p5()
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(tokens.colors().text_muted)
                                .child(rust_i18n::t!("detail.conditions").to_string()),
                        )
                        .children(overview.conditions.into_iter().map(|condition| {
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::empty()
                                        .path(icon::health(condition.level))
                                        .size(px(8.))
                                        .text_color(tokens.colors().health(condition.level)),
                                )
                                .child(
                                    div()
                                        .w(px(140.))
                                        .flex_shrink_0()
                                        .text_size(px(12.))
                                        .font_family("monospace")
                                        .text_color(tokens.colors().text_secondary)
                                        .truncate()
                                        .child(condition.kind),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(11.5))
                                        .text_color(tokens.colors().text_muted)
                                        .truncate()
                                        .child(match condition.reason.is_empty() {
                                            true => condition.status,
                                            false => condition.reason,
                                        }),
                                )
                        })),
                )
            })
            .into_any_element()
    }

    /// The resources Argo CD reports for the Application.
    ///
    /// This is deliberately a flat, virtualized list. The Application CR has
    /// a flat `status.resources`; inventing ownership edges would be less
    /// honest than the data, and drawing one element per resource would fail
    /// on the applications for which this view matters most.
    fn resources(&self, object: &Object, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let application = kirikumo_ui::gitops::Application::from_object(object);
        let resources = {
            let store = self.store.read(cx);
            let current_server = store.current_server();
            application
                .resources_for_display()
                .into_iter()
                .map(|resource| {
                    let namespaced = store
                        .resource(&resource.key())
                        .map(|served| served.namespaced);
                    let target = resource.target(&application, current_server, namespaced);
                    (resource, target)
                })
                .collect::<Vec<_>>()
        };
        if resources.is_empty() {
            return self.notice(
                rust_i18n::t!("detail.resources_empty").to_string(),
                false,
                cx,
            );
        }
        let count = resources.len();
        let row_tokens = tokens.clone();
        let this = cx.entity();
        let rows = uniform_list("gitops-resources", count, move |range, _window, cx| {
            this.update(cx, |_this, cx| {
                range
                    .map(|index| {
                        let (resource, target) = &resources[index];
                        let target = target.clone();
                        let level = resource.level();
                        let qualified = resource.key().qualified();
                        let place = match resource.namespace.as_deref() {
                            Some(namespace) => format!("{qualified} · {namespace}"),
                            None => qualified,
                        };
                        h_flex()
                            .id(("gitops-resource", index))
                            .h(px(36.))
                            .w_full()
                            .px_3()
                            .gap_2()
                            .items_center()
                            .border_b_1()
                            .border_color(row_tokens.colors().border_subtle)
                            .when(target.is_some(), |this| {
                                this.cursor_pointer()
                                    .hover(|this| this.bg(row_tokens.colors().row_hover()))
                            })
                            .child(
                                Icon::empty()
                                    .path(icon::health(level))
                                    .size(px(8.))
                                    .text_color(row_tokens.colors().health(level)),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_family("monospace")
                                            .text_color(row_tokens.colors().text_primary)
                                            .truncate()
                                            .child(resource.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(row_tokens.colors().text_muted)
                                            .truncate()
                                            .child(place),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(70.))
                                    .flex_shrink_0()
                                    .text_size(px(10.5))
                                    .text_color(row_tokens.colors().text_secondary)
                                    .truncate()
                                    .child(resource.sync.clone()),
                            )
                            .child(
                                div()
                                    .w(px(70.))
                                    .flex_shrink_0()
                                    .text_size(px(10.5))
                                    .text_color(row_tokens.colors().health(level))
                                    .truncate()
                                    .child(resource.health.clone()),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(target) = target.clone() {
                                    cx.emit(DetailEvent::Navigate(target));
                                }
                            }))
                    })
                    .collect()
            })
        })
        .flex_1()
        .w_full();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(26.))
                    .w_full()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(tokens.colors().border_strong)
                    .text_size(px(10.))
                    .text_color(tokens.colors().text_muted)
                    .child(div().w(px(8.)))
                    .child(div().flex_1().child("NAME · KIND · NAMESPACE"))
                    .child(div().w(px(70.)).child("SYNC"))
                    .child(div().w(px(70.)).child("HEALTH")),
            )
            .child(rows)
            .into_any_element()
    }

    /// Start a forward, remembering why it could not be if it could not.
    fn start_forward(&mut self, remote: u16, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else {
            return;
        };
        self.forward_error = self
            .store
            .update(cx, |store, cx| store.start_forward(key, remote, cx))
            .err();
        cx.notify();
    }

    /// The pod's ports, and the forwards open to them.
    ///
    /// Not a write in K6's sense — nothing in the cluster changes — but it
    /// opens a port on this machine, so it is one deliberate click on a
    /// chip that names the port, and the strip says what is open.
    fn forwards(&self, object: &Object, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let key = self.key.clone();
        let ports = detail::container_ports(object);
        let active: Vec<(u16, u16, usize, Option<String>)> = key
            .as_ref()
            .map(|key| {
                self.store
                    .read(cx)
                    .forwards_for(key)
                    .map(|forward| {
                        (
                            forward.remote,
                            forward.local(),
                            forward.forwarder.open_connections(),
                            forward.forwarder.last_error(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        v_flex()
            .w_full()
            .gap_1p5()
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(tokens.colors().text_muted)
                    .child(rust_i18n::t!("detail.forward").to_string()),
            )
            .when(ports.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(tokens.colors().text_muted)
                        .child(rust_i18n::t!("detail.forward_none").to_string()),
                )
            })
            .when(!ports.is_empty(), |this| {
                this.child(h_flex().gap_1().flex_wrap().children(ports.into_iter().map(
                    |(remote, label)| {
                        let forwarding = active.iter().any(|(port, ..)| *port == remote);
                        self.button(
                            "forward-port",
                            label,
                            !forwarding,
                            false,
                            cx,
                            move |this, _, cx| this.start_forward(remote, cx),
                        )
                    },
                )))
            })
            .children(active.into_iter().map(|(remote, local, open, error)| {
                let address = format!("localhost:{local}");
                let copied = address.clone();
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_family("monospace")
                            .text_color(tokens.colors().text_secondary)
                            .truncate()
                            .child(format!("{address} → {remote}")),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(match error.is_some() {
                                true => tokens.colors().status_error,
                                false => tokens.colors().text_muted,
                            })
                            .child(match error {
                                Some(error) => error,
                                None => {
                                    rust_i18n::t!("detail.forward_open", count = open).to_string()
                                }
                            }),
                    )
                    .child(self.button(
                        "forward-copy",
                        rust_i18n::t!("detail.forward_copy").to_string(),
                        true,
                        false,
                        cx,
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                        },
                    ))
                    .child(self.button(
                        "forward-stop",
                        rust_i18n::t!("detail.forward_stop").to_string(),
                        true,
                        false,
                        cx,
                        move |this, _, cx| {
                            if let Some(key) = this.key.clone() {
                                this.store
                                    .update(cx, |store, cx| store.stop_forward(&key, remote, cx));
                            }
                        },
                    ))
            }))
            .children(self.forward_error.clone().map(|error| {
                div()
                    .text_size(px(11.5))
                    .text_color(tokens.colors().status_error)
                    .child(error)
            }))
            .into_any_element()
    }

    /// The Events tab.
    fn events(&self, object: &Object, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let now = Utc::now();
        let fetch = self.store.read(cx).events(&object.meta.uid);
        let events = fetch.and_then(|fetch| fetch.value()).cloned();
        let loading = fetch.is_some_and(|fetch| fetch.is_loading());
        let error = fetch.and_then(|fetch| fetch.error()).map(str::to_string);

        let Some(events) = events.filter(|events| !events.is_empty()) else {
            return match (error, loading) {
                (Some(error), _) => self.notice(error, true, cx),
                (None, true) => crate::skeleton::detail(cx),
                (None, false) => {
                    self.notice(rust_i18n::t!("detail.events_empty").to_string(), false, cx)
                }
            };
        };

        v_flex()
            .id("events")
            .size_full()
            .px_4()
            .py_3()
            .gap_2p5()
            .overflow_y_scroll()
            .children(events.into_iter().map(|event| {
                let level = match event.is_warning() {
                    true => kirikumo_kube::Level::Attention,
                    false => kirikumo_kube::Level::Ok,
                };
                v_flex()
                    .w_full()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::empty()
                                    .path(icon::health(level))
                                    .size(px(8.))
                                    .text_color(tokens.colors().health(level)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_family("monospace")
                                    .text_color(tokens.colors().text_secondary)
                                    .child(event.reason.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.))
                                    .text_color(tokens.colors().text_muted)
                                    .child(match event.count > 1 {
                                        true => format!(
                                            "{} · ×{}",
                                            time::age(event.last, now),
                                            event.count
                                        ),
                                        false => time::age(event.last, now),
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .pl(px(16.))
                            .text_size(px(12.))
                            .text_color(tokens.colors().text_secondary)
                            .child(event.message.clone()),
                    )
            }))
            .into_any_element()
    }

    /// The YAML tab: the object as the apiserver holds it, in the toolkit's
    /// editor — read-only until *Edit*, and then the reader's until *Apply*
    /// or *Cancel*.
    fn yaml(&mut self, object: &Object, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let Some(key) = self.key.clone() else {
            return div().into_any_element();
        };
        // Refilled when the object or its version changes, and never while
        // the text is the reader's: a watch event landing mid-edit must not
        // throw their edit away.
        let holds = (key.clone(), object.meta.resource_version.clone());
        if !self.editing && self.editor_holds.as_ref() != Some(&holds) {
            let text = yaml::to_yaml(&object.raw);
            self.editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
            self.editor_holds = Some(holds);
        }

        let editing = self.editing;
        let armed = self.pending.action() == Some(Action::Apply);
        let may_apply = self.permission(Action::Apply, cx) == Some(true);
        let writing = self
            .store
            .read(cx)
            .write(&key)
            .is_some_and(|w| w.is_loading());
        let name = object.meta.name.clone();

        let strip = h_flex()
            .w_full()
            .px_3()
            .py_1p5()
            .gap_1()
            .flex_shrink_0()
            .items_center()
            .justify_end()
            .when(!editing, |this| {
                this.child(self.button(
                    "edit",
                    rust_i18n::t!("action.edit").to_string(),
                    may_apply && !writing,
                    false,
                    cx,
                    |this, _, cx| {
                        this.editing = true;
                        cx.notify();
                    },
                ))
            })
            .when(editing && !armed, |this| {
                this.child(self.button(
                    "apply",
                    rust_i18n::t!("action.apply").to_string(),
                    !writing,
                    false,
                    cx,
                    |this, _, cx| this.arm(Action::Apply, cx),
                ))
            })
            .when(editing && armed, |this| {
                this.child(self.button(
                    "confirm-apply",
                    confirm_label(Action::Apply, &name, None),
                    !writing,
                    false,
                    cx,
                    |this, window, cx| this.confirm(window, cx),
                ))
            })
            .when(editing, |this| {
                this.child(self.button(
                    "cancel-edit",
                    rust_i18n::t!("action.cancel").to_string(),
                    true,
                    false,
                    cx,
                    |this, _, cx| {
                        this.editing = false;
                        this.pending = Pending::Idle;
                        // Forget what the editor holds, so the next frame
                        // refills it from the object.
                        this.editor_holds = None;
                        cx.notify();
                    },
                ))
            });

        v_flex()
            .size_full()
            .child(strip)
            .child(
                div().flex_1().min_h_0().w_full().px_2().pb_2().child(
                    Editor::new(&self.editor)
                        .readonly(!editing)
                        .bordered(editing)
                        .h(relative(1.))
                        .text_size(px(12.)),
                ),
            )
            .when(!editing, |this| {
                // A hint that the text is the apiserver's, not the reader's.
                this.child(
                    div()
                        .px_3()
                        .pb_1p5()
                        .text_size(px(10.5))
                        .text_color(tokens.colors().text_muted)
                        .child(rust_i18n::t!("detail.yaml_readonly").to_string()),
                )
            })
            .into_any_element()
    }

    /// Whether this login may do an action on the object shown, if the
    /// cluster has said.
    fn permission(&self, action: Action, cx: &App) -> Option<bool> {
        let (key, namespace, _) = self.key.as_ref()?;
        let store = self.store.read(cx);
        let namespaced = store
            .resource(key)
            .is_none_or(|resource| resource.namespaced);
        let namespace = namespace.as_deref().filter(|_| namespaced);
        store.permission(key, namespace, action.verb())
    }

    /// The first gesture.
    fn arm(&mut self, action: Action, cx: &mut Context<Self>) {
        let current = self
            .object(cx)
            .map(|object| actions::current_replicas(&object))
            .unwrap_or(1);
        self.pending = Pending::arm(action, current);
        cx.notify();
    }

    /// Back to nothing armed.
    fn disarm(&mut self, cx: &mut Context<Self>) {
        self.pending = Pending::Idle;
        cx.notify();
    }

    /// The second gesture: build the write and send it.
    fn confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.pending.action() else {
            return;
        };
        if !self.pending.can_confirm() {
            return;
        }
        let Some(key) = self.key.clone() else {
            return;
        };
        let write = match action {
            Action::Delete => Write::Delete,
            Action::Sync => Write::Patch(actions::sync()),
            Action::Scale => match self.pending.replicas() {
                Some(count) => Write::Patch(actions::scale(count)),
                None => return,
            },
            Action::Restart => Write::Patch(actions::restart(Utc::now())),
            Action::Cordon => Write::Patch(actions::schedulable(true)),
            Action::Uncordon => Write::Patch(actions::schedulable(false)),
            Action::Drain => Write::Drain,
            Action::Apply => {
                let text = self.editor.read(cx).value().to_string();
                match actions::apply(&text) {
                    Ok(patch) => Write::Patch(patch),
                    Err(error) => {
                        // Refused here, before anything is sent: the reason
                        // goes where the apiserver's would.
                        self.apply_error = Some(kirikumo_ui::fetch::describe(&error));
                        self.pending = Pending::Idle;
                        cx.notify();
                        return;
                    }
                }
            }
        };
        self.apply_error = None;
        self.pending = Pending::Idle;
        // A cluster-scoped kind is written without a namespace, whatever the
        // detail was opened with.
        let namespaced = self
            .store
            .read(cx)
            .resource(&key.0)
            .is_none_or(|resource| resource.namespaced);
        let target = (
            key.0.clone(),
            key.1.clone().filter(|_| namespaced),
            key.2.clone(),
        );
        self.editing = false;
        self.editor_holds = None;
        self.store
            .update(cx, |store, cx| store.perform(target, write, cx));
        cx.notify();
    }

    /// A button in the footer or the YAML strip.
    ///
    /// Greyed rather than hidden when it cannot be pressed: a control that
    /// vanishes leaves the reader wondering whether the thing can be done at
    /// all, and one that is grey with a tooltip says exactly why not.
    fn button(
        &self,
        id: &'static str,
        label: String,
        enabled: bool,
        destructive: bool,
        cx: &mut Context<Self>,
        act: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let tokens = Tokens::global(cx).clone();
        let tip = rust_i18n::t!("action.forbidden").to_string();
        div()
            .id(id)
            .px_2p5()
            .py_1()
            .flex_shrink_0()
            .rounded(px(tokens.radius.control()))
            .text_size(px(11.5))
            .when(enabled && destructive, |this| {
                this.cursor_pointer()
                    .bg(tokens.colors().status_error.opacity(0.18))
                    .text_color(tokens.colors().status_error)
                    .hover(|this| this.bg(tokens.colors().status_error.opacity(0.3)))
            })
            .when(enabled && !destructive, |this| {
                this.cursor_pointer()
                    .bg(tokens.colors().bg_surface)
                    .text_color(tokens.colors().text_primary)
                    .hover(|this| this.bg(tokens.colors().surface_hover()))
            })
            .when(!enabled, |this| {
                this.bg(tokens.colors().bg_surface.opacity(0.5))
                    .text_color(tokens.colors().text_muted)
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            .child(label)
            .when(enabled, |this| {
                this.on_click(cx.listener(move |this, _, window, cx| act(this, window, cx)))
            })
    }

    /// The actions strip at the foot of the panel: the first gesture, then
    /// the second, then what the apiserver said.
    fn footer(&self, object: &Object, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tokens = Tokens::global(cx).clone();
        let key = self.key.clone()?;
        let resource = self.store.read(cx).resource(&key.0).cloned()?;
        // Apply lives on the YAML tab, beside the text it applies.
        let available: Vec<Action> = actions::available(&resource, object)
            .into_iter()
            .filter(|action| *action != Action::Apply)
            .collect();
        if available.is_empty() {
            return None;
        }
        let write = self.store.read(cx).write(&key).cloned();
        let working = write.as_ref().is_some_and(|write| write.is_loading());
        // What a landed write had to say: nothing for most, a drain's
        // `3 evicted · 1 skipped` for a drain.
        let landed = write
            .as_ref()
            .and_then(|write| write.value())
            .filter(|line| !line.is_empty())
            .cloned();
        let refused = self.apply_error.clone().or_else(|| {
            write
                .as_ref()
                .and_then(|write| write.error())
                .map(str::to_string)
        });
        let name = object.meta.name.clone();
        let armed = self.pending.action();

        let controls: Vec<AnyElement> = match armed {
            None => available
                .iter()
                .map(|action| {
                    let action = *action;
                    let allowed = self.permission(action, cx) == Some(true);
                    self.button(
                        match action {
                            Action::Sync => "act-sync",
                            Action::Scale => "act-scale",
                            Action::Restart => "act-restart",
                            Action::Cordon => "act-cordon",
                            Action::Uncordon => "act-uncordon",
                            Action::Drain => "act-drain",
                            Action::Apply => "act-apply",
                            Action::Delete => "act-delete",
                        },
                        rust_i18n::t!(action.label_key()).to_string(),
                        allowed && !working,
                        action.is_destructive(),
                        cx,
                        move |this, _, cx| this.arm(action, cx),
                    )
                    .into_any_element()
                })
                .collect(),
            Some(action) => {
                let mut controls = Vec::new();
                if action == Action::Scale {
                    controls.push(
                        div()
                            .w(px(72.))
                            .flex_shrink_0()
                            .child(Input::new(&self.replicas))
                            .into_any_element(),
                    );
                }
                controls.push(
                    self.button(
                        "confirm",
                        confirm_label(action, &name, self.pending.replicas()),
                        self.pending.can_confirm() && !working,
                        action.is_destructive(),
                        cx,
                        |this, window, cx| this.confirm(window, cx),
                    )
                    .into_any_element(),
                );
                controls.push(
                    self.button(
                        "cancel",
                        rust_i18n::t!("action.cancel").to_string(),
                        true,
                        false,
                        cx,
                        |this, _, cx| this.disarm(cx),
                    )
                    .into_any_element(),
                );
                controls
            }
        };

        Some(
            v_flex()
                .w_full()
                .flex_shrink_0()
                .border_t_1()
                .border_color(tokens.colors().border_subtle)
                .child(
                    h_flex()
                        .w_full()
                        .px_3()
                        .py_2()
                        .gap_1p5()
                        .items_center()
                        .flex_wrap()
                        .children(controls)
                        .when(working, |this| {
                            this.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(tokens.colors().text_muted)
                                    .child(rust_i18n::t!("action.working").to_string()),
                            )
                        }),
                )
                .children(refused.map(|error| {
                    div()
                        .px_3()
                        .pb_2()
                        .text_size(px(11.5))
                        .text_color(tokens.colors().status_error)
                        .child(error)
                }))
                .children(landed.map(|line| {
                    div()
                        .px_3()
                        .pb_2()
                        .text_size(px(11.5))
                        .text_color(tokens.colors().text_secondary)
                        .child(line)
                }))
                .into_any_element(),
        )
    }

    /// Run what is in the command field, in the chosen container.
    ///
    /// The command is the reader's own words, typed, which is the deliberate
    /// act; the run itself is one press. It is not a K6 write to the cluster
    /// — nothing in the apiserver changes — but it can change a container,
    /// so the review for `pods/exec` gates it like any write.
    fn run_command(&mut self, cx: &mut Context<Self>) {
        let line = self.command.read(cx).value().trim().to_string();
        if line.is_empty() {
            return;
        }
        let Some(key) = self.key.clone() else {
            return;
        };
        let Some(object) = self.object(cx) else {
            return;
        };
        let Some(namespace) = object.meta.namespace.clone() else {
            return;
        };
        if self.store.read(cx).exec_permission(Some(&namespace)) != Some(true) {
            return;
        }
        let containers = self.containers(cx);
        let mut request = ExecRequest::shell(namespace, object.meta.name.clone(), &line);
        if let Some(container) = self
            .container
            .clone()
            .or_else(|| containers.first().cloned())
        {
            request = request.container(container);
        }
        self.store
            .update(cx, |store, cx| store.exec(key, request, cx));
        cx.notify();
    }

    /// The Run tab: a command, and what it said.
    fn run_tab(&mut self, object: &Object, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let containers = self.containers(cx);
        let current = self
            .container
            .clone()
            .or_else(|| containers.first().cloned())
            .unwrap_or_default();
        let allowed = self
            .store
            .read(cx)
            .exec_permission(object.meta.namespace.as_deref())
            == Some(true);
        let run = self
            .key
            .as_ref()
            .and_then(|key| self.store.read(cx).run(key).cloned());
        let working = run.as_ref().is_some_and(|run| run.is_loading());
        let refused = run.as_ref().and_then(|run| run.error()).map(str::to_string);
        let output = run.as_ref().and_then(|run| run.value()).cloned();

        let toolbar = h_flex()
            .w_full()
            .px_3()
            .py_1p5()
            .gap_1()
            .flex_shrink_0()
            .items_center()
            .children(containers.into_iter().enumerate().map(|(index, name)| {
                let selected = name == current;
                let picked = name.clone();
                div()
                    .id(("run-container", index))
                    .px_2()
                    .py_0p5()
                    .rounded(px(tokens.radius.control()))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .font_family("monospace")
                    .when(selected, |this| {
                        this.bg(tokens.colors().row_active())
                            .text_color(tokens.colors().text_primary)
                    })
                    .when(!selected, |this| {
                        this.text_color(tokens.colors().text_muted)
                    })
                    .hover(|this| this.bg(tokens.colors().row_hover()))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.container = Some(picked.clone());
                        cx.notify();
                    }))
            }))
            .child(div().flex_1().child(Input::new(&self.command)))
            .child(self.button(
                "run",
                rust_i18n::t!("detail.run_button").to_string(),
                allowed && !working,
                false,
                cx,
                |this, _, cx| this.run_command(cx),
            ));

        // stdout, then stderr, then the status: the two streams are separate
        // channels on the wire and the apiserver does not order them against
        // each other, so they are not interleaved here either.
        let mut lines: Vec<(SharedString, bool)> = Vec::new();
        if let Some(output) = &output {
            lines.extend(
                output
                    .stdout
                    .lines()
                    .map(|line| (SharedString::from(line.to_string()), false)),
            );
            if !output.stderr.trim().is_empty() {
                lines.push((
                    SharedString::from(format!("— {} —", rust_i18n::t!("detail.run_stderr"))),
                    true,
                ));
                lines.extend(
                    output
                        .stderr
                        .lines()
                        .map(|line| (SharedString::from(line.to_string()), true)),
                );
            }
        }
        let status: Option<(String, bool)> = output.as_ref().map(|output| match &output.failure {
            Some(failure) => (failure.clone(), true),
            None => match output.exit_code {
                Some(code) => (
                    rust_i18n::t!("detail.run_exit", code = code).to_string(),
                    code != 0,
                ),
                None => (rust_i18n::t!("detail.run_no_exit").to_string(), true),
            },
        });

        let body: AnyElement = if working {
            crate::skeleton::detail(cx)
        } else if let Some(error) = refused {
            self.notice(error, true, cx)
        } else if lines.is_empty() && output.is_some() {
            self.notice(rust_i18n::t!("detail.run_empty").to_string(), false, cx)
        } else if lines.is_empty() {
            div().into_any_element()
        } else {
            let colors = *tokens.colors();
            uniform_list("run-output", lines.len(), move |range, _window, _cx| {
                range
                    .map(|index| {
                        let (line, is_err) = lines
                            .get(index)
                            .cloned()
                            .unwrap_or((SharedString::default(), false));
                        div()
                            .w_full()
                            .h(LINE_HEIGHT)
                            .px_3()
                            .text_size(px(12.))
                            .font_family("monospace")
                            .text_color(match is_err {
                                true => colors.status_error,
                                false => colors.text_secondary,
                            })
                            .child(line)
                    })
                    .collect()
            })
            .size_full()
            .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(tokens.colors().bg_terminal)
            .child(toolbar)
            .when(!allowed, |this| {
                this.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_size(px(11.))
                        .text_color(tokens.colors().text_muted)
                        .child(rust_i18n::t!("action.forbidden").to_string()),
                )
            })
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(status.map(|(text, bad)| {
                div()
                    .px_3()
                    .py_1p5()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .font_family("monospace")
                    .text_color(match bad {
                        true => tokens.colors().status_error,
                        false => tokens.colors().text_muted,
                    })
                    .child(text)
            }))
            .into_any_element()
    }

    /// Attach a shell to the pod on screen, in the container picked.
    fn attach_shell(&mut self, cx: &mut Context<Self>) {
        let Some((key, object)) = self.key.clone().zip(self.object(cx)) else {
            return;
        };
        let Some(namespace) = object.meta.namespace.clone() else {
            return;
        };
        if self.store.read(cx).exec_permission(Some(&namespace)) != Some(true) {
            return;
        }
        let containers = self.containers(cx);
        let mut request = ExecRequest::attach(namespace, object.meta.name.clone());
        if let Some(container) = self
            .container
            .clone()
            .or_else(|| containers.first().cloned())
        {
            request = request.container(container);
        }
        let (cols, rows) = self.shell_cells.unwrap_or(SHELL_DEFAULT);
        self.store.update(cx, |store, cx| {
            store.attach_shell(key, request, cols, rows, cx)
        });
        cx.notify();
    }

    /// A keystroke on the Shell tab: to the shell, as bytes.
    fn type_into_shell(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(bytes) = terminal::keystroke_bytes(&event.keystroke) else {
            return;
        };
        cx.stop_propagation();
        self.store
            .update(cx, |store, _| store.type_into_shell(bytes));
    }

    /// The shell panel was laid out at `cols` by `rows`.
    ///
    /// Called from the frame, after the layout has happened; only a change
    /// does anything, so a frame that changed nothing costs a comparison.
    fn shell_measured(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        if self.shell_cells == Some((cols, rows)) {
            return;
        }
        self.shell_cells = Some((cols, rows));
        self.store
            .update(cx, |store, cx| store.resize_shell(cols, rows, cx));
        cx.notify();
    }

    /// The Shell tab: a container, and a terminal into it.
    fn shell_tab(
        &mut self,
        object: &Object,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let containers = self.containers(cx);
        let allowed = self
            .store
            .read(cx)
            .exec_permission(object.meta.namespace.as_deref())
            == Some(true);
        let session = self
            .key
            .as_ref()
            .zip(self.store.read(cx).shell())
            .filter(|(key, shell)| **key == shell.pod);
        let attached = session
            .as_ref()
            .map(|(_, shell)| (shell.status.clone(), shell.container.clone()));
        let current = attached
            .as_ref()
            .and_then(|(_, container)| container.clone())
            .or_else(|| self.container.clone())
            .or_else(|| containers.first().cloned())
            .unwrap_or_default();
        let live = matches!(
            attached.as_ref().map(|(status, _)| status),
            Some(ShellStatus::Connecting | ShellStatus::Open)
        );

        let toolbar = h_flex()
            .w_full()
            .px_3()
            .py_1p5()
            .gap_1()
            .flex_shrink_0()
            .items_center()
            .children(containers.into_iter().enumerate().map(|(index, name)| {
                let selected = name == current;
                let picked = name.clone();
                div()
                    .id(("shell-container", index))
                    .px_2()
                    .py_0p5()
                    .rounded(px(tokens.radius.control()))
                    .text_size(px(11.))
                    .font_family("monospace")
                    .when(selected, |this| {
                        this.bg(tokens.colors().row_active())
                            .text_color(tokens.colors().text_primary)
                    })
                    .when(!selected, |this| {
                        this.text_color(tokens.colors().text_muted)
                    })
                    // The container cannot change under a shell that is
                    // in it; detach first.
                    .when(!live, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(tokens.colors().row_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.container = Some(picked.clone());
                                cx.notify();
                            }))
                    })
                    .child(name)
            }))
            .child(div().flex_1())
            .child(match live {
                true => self.button(
                    "shell-detach",
                    rust_i18n::t!("detail.shell_detach").to_string(),
                    true,
                    false,
                    cx,
                    |this, _, cx| {
                        this.store.update(cx, |store, _| store.detach_shell());
                        cx.notify();
                    },
                ),
                false => self.button(
                    "shell-attach",
                    rust_i18n::t!("detail.shell_attach").to_string(),
                    allowed,
                    false,
                    cx,
                    |this, window, cx| {
                        this.attach_shell(cx);
                        window.focus(&this.shell_focus, cx);
                    },
                ),
            });

        let body: AnyElement = match attached.as_ref().map(|(status, _)| status) {
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .px_6()
                .child(
                    div()
                        .max_w(px(360.))
                        .text_size(px(11.5))
                        .text_color(tokens.colors().text_muted)
                        .text_center()
                        .child(rust_i18n::t!("detail.shell_hint").to_string()),
                )
                .into_any_element(),
            Some(ShellStatus::Failed(why)) => self.notice(why.clone(), true, cx),
            Some(status) => {
                let footnote = match status {
                    ShellStatus::Connecting => {
                        Some((rust_i18n::t!("detail.shell_connecting").to_string(), false))
                    }
                    ShellStatus::Closed => {
                        Some((rust_i18n::t!("detail.shell_closed").to_string(), false))
                    }
                    _ => None,
                };
                v_flex()
                    .size_full()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(self.shell_screen(window, cx)),
                    )
                    .children(footnote.map(|(text, bad)| self.notice(text, bad, cx)))
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .bg(tokens.colors().bg_terminal)
            .child(toolbar)
            .when(!allowed, |this| {
                this.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_size(px(11.))
                        .text_color(tokens.colors().text_muted)
                        .child(rust_i18n::t!("action.forbidden").to_string()),
                )
            })
            .child(div().flex_1().min_h_0().w_full().child(body))
            .into_any_element()
    }

    /// The shell's screen: one styled line per row, and a ruler underneath
    /// that measures the panel in cells so the shell can be told its size.
    fn shell_screen(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let colors = *tokens.colors();
        let rows: Vec<Vec<terminal::Span>> = self
            .store
            .read(cx)
            .shell()
            .map(|shell| {
                shell
                    .screen
                    .rows_of_cells()
                    .iter()
                    .map(|row| terminal::spans(row))
                    .collect()
            })
            .unwrap_or_default();
        let focused = self.shell_focus.is_focused(window);
        let mono = font("monospace");
        let this = cx.entity().downgrade();

        // The ruler: laid out under the rows at the panel's full size, it
        // learns the bounds every frame and reports them in cells. Text
        // width is measured rather than assumed, since the monospace font is
        // whatever the platform resolved it to.
        let ruler = canvas(
            move |bounds, window, cx| {
                let probe = TextRun {
                    len: 10,
                    font: mono.clone(),
                    color: colors.text_primary,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let width = window
                    .text_system()
                    .shape_line("MMMMMMMMMM".into(), SHELL_FONT_SIZE, &[probe], None)
                    .width;
                let cell = f32::from(width) / 10.;
                if cell <= 0. {
                    return;
                }
                let cols = (f32::from(bounds.size.width) / cell).floor().max(1.) as u16;
                let rows = (f32::from(bounds.size.height) / f32::from(LINE_HEIGHT))
                    .floor()
                    .max(1.) as u16;
                this.update(cx, |this, cx| this.shell_measured(cols, rows, cx))
                    .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        div()
            .id("shell-screen")
            .relative()
            .size_full()
            .track_focus(&self.shell_focus)
            .key_context("Shell")
            .cursor_text()
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.type_into_shell(event, cx)),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.shell_focus, cx);
                cx.notify();
            }))
            .child(ruler)
            .child(
                v_flex()
                    .relative()
                    .size_full()
                    .px_3()
                    .overflow_hidden()
                    .font_family("monospace")
                    .text_size(SHELL_FONT_SIZE)
                    .line_height(LINE_HEIGHT)
                    .children(rows.into_iter().enumerate().map(|(index, spans)| {
                        let text: String = spans.iter().map(|span| span.text.as_str()).collect();
                        let runs = spans
                            .iter()
                            .map(|span| shell_run(span, &colors, focused))
                            .collect();
                        div()
                            .id(("shell-row", index))
                            .h(LINE_HEIGHT)
                            .whitespace_nowrap()
                            .child(StyledText::new(text).with_runs(runs))
                    })),
            )
            .into_any_element()
    }

    /// The Logs tab: what to read, and then the reading of it.
    fn logs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = Tokens::global(cx).clone();
        let subject = self.object(cx);
        let indirect = subject
            .as_ref()
            .is_some_and(|object| self.indirect_log_namespace(object).is_some());
        let node = self
            .key
            .as_ref()
            .is_some_and(|(resource, _, _)| is_node(resource));
        let pods = self.log_pods(cx);
        let current_pod = self
            .log_pod(cx)
            .filter(|_| indirect)
            .map(|pod| (pod.meta.namespace, pod.meta.name));
        let containers = self.log_containers(cx);
        let current = self
            .container
            .clone()
            .filter(|selected| containers.contains(selected))
            .or_else(|| containers.first().cloned())
            .unwrap_or_default();
        let fetch = self
            .log_request(cx)
            .map(|request| Store::log_key(&request))
            .and_then(|key| self.store.read(cx).logs(&key).cloned());
        let lines = fetch
            .as_ref()
            .and_then(|fetch| fetch.value())
            .cloned()
            .unwrap_or_default();
        let error = fetch
            .as_ref()
            .and_then(|fetch| fetch.error())
            .map(str::to_string);
        let loading = fetch.as_ref().is_some_and(|fetch| fetch.is_loading());
        let query = self.find.read(cx).value().to_string();
        let visible = logs::matching(&lines, &query);

        // Following means the end stays in view — until the reader takes
        // hold of the list. Checked only when something arrived, because
        // that is the only moment following would move anything: if the
        // list was not at its end when the new lines came, the reader
        // scrolled up to read something, and following drops rather than
        // yanking them back down. The end is where the scroll was at the
        // last layout, so a list we just scrolled ourselves gets a frame's
        // grace before it can count as "the reader moved it".
        let arrived = lines.len() != self.last_lines && !visible.is_empty();
        if self.following && arrived {
            match self.scroll.is_scrolled_to_end() {
                Some(false) if !self.scrolled_last_frame => self.following = false,
                _ => self.scroll.scroll_to_bottom(),
            }
        }
        self.scrolled_last_frame = self.following && arrived;
        self.last_lines = lines.len();

        let pod_picker = (indirect && !pods.is_empty()).then(|| {
            h_flex()
                .w_full()
                .h(LOG_TOOLBAR_HEIGHT)
                .min_h(LOG_TOOLBAR_HEIGHT)
                .px_3()
                .pt_1p5()
                .gap_1()
                .flex_shrink_0()
                .items_center()
                .overflow_x_scrollbar()
                .child(
                    div()
                        .mr_1()
                        .flex_shrink_0()
                        .text_size(px(10.5))
                        .text_color(tokens.colors().text_muted)
                        .child(rust_i18n::t!("detail.pod").to_string()),
                )
                .children(pods.into_iter().enumerate().map(|(index, pod)| {
                    let name = pod.meta.name;
                    let namespace = pod.meta.namespace;
                    let selected =
                        current_pod
                            .as_ref()
                            .is_some_and(|(current_namespace, current_name)| {
                                current_namespace == &namespace && current_name == &name
                            });
                    let picked = (namespace.clone(), name.clone());
                    let label = match (node, namespace) {
                        (true, Some(namespace)) => format!("{namespace}/{name}"),
                        _ => name,
                    };
                    div()
                        .id(("log-pod", index))
                        .px_2()
                        .py_0p5()
                        .flex_shrink_0()
                        .rounded(px(tokens.radius.control()))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .font_family("monospace")
                        .when(selected, |this| {
                            this.bg(tokens.colors().row_active())
                                .text_color(tokens.colors().text_primary)
                        })
                        .when(!selected, |this| {
                            this.text_color(tokens.colors().text_muted)
                        })
                        .hover(|this| this.bg(tokens.colors().row_hover()))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pod = Some(picked.clone());
                            this.container = None;
                            this.stop_following(cx);
                            this.reload_log(cx);
                        }))
                }))
        });

        let controls = h_flex()
            .w_full()
            .h(LOG_TOOLBAR_HEIGHT)
            .min_h(LOG_TOOLBAR_HEIGHT)
            .px_3()
            .py_1p5()
            .gap_1()
            .flex_shrink_0()
            .items_center()
            .overflow_x_scrollbar()
            .children(containers.into_iter().enumerate().map(|(index, name)| {
                let selected = name == current;
                let picked = name.clone();
                div()
                    .id(("container", index))
                    .px_2()
                    .py_0p5()
                    .rounded(px(tokens.radius.control()))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .font_family("monospace")
                    .when(selected, |this| {
                        this.bg(tokens.colors().row_active())
                            .text_color(tokens.colors().text_primary)
                    })
                    .when(!selected, |this| {
                        this.text_color(tokens.colors().text_muted)
                    })
                    .hover(|this| this.bg(tokens.colors().row_hover()))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.container = Some(picked.clone());
                        this.reload_log(cx);
                    }))
            }))
            .child(div().flex_1())
            .child(self.toggle(
                "follow",
                rust_i18n::t!("detail.follow").to_string(),
                self.following,
                cx,
                |this, cx| {
                    this.following = !this.following;
                    if this.following {
                        // *Follow* pressed is *jump to the end* as well: the
                        // reader who scrolled up and is done reading wants
                        // to be back where the new lines are.
                        this.scroll.scroll_to_bottom();
                    }
                    this.reload_log(cx);
                },
            ))
            .child(self.toggle(
                "previous",
                rust_i18n::t!("detail.previous").to_string(),
                self.previous,
                cx,
                |this, cx| {
                    this.previous = !this.previous;
                    this.reload_log(cx);
                },
            ))
            .child(
                div()
                    .w(px(150.))
                    .child(Input::new(&self.find).cleanable(true)),
            )
            // How much of the log the find box is hiding, which is the one
            // number that stops a filtered log being mistaken for a short one.
            .when(!query.trim().is_empty(), |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(10.5))
                        .text_color(tokens.colors().text_muted)
                        .child(format!("{}/{}", visible.len(), lines.len())),
                )
            });

        let toolbar = v_flex()
            .w_full()
            .flex_shrink_0()
            .bg(tokens.colors().bg_terminal)
            .border_b_1()
            .border_color(tokens.colors().border_subtle)
            .children(pod_picker)
            .child(controls);

        let indirect_pods_fetch = subject.and_then(|object| {
            let namespace = self.indirect_log_namespace(&object)?;
            let pod_key = kirikumo_kube::ResourceKey::new("", "Pod");
            self.store
                .read(cx)
                .list(&pod_key, namespace.as_deref())
                .cloned()
        });
        let indirect_pods_loading = indirect
            && indirect_pods_fetch
                .as_ref()
                .is_none_or(|fetch| fetch.is_loading());
        let indirect_pods_error = indirect_pods_fetch
            .as_ref()
            .and_then(|fetch| fetch.error())
            .map(str::to_string);

        let body: AnyElement = if !visible.is_empty() {
            let colors = *tokens.colors();
            // Cloned into the closure rather than read from the store on each
            // frame: the closure outlives this borrow, and a log's lines are
            // `Arc`-free `String`s the list only ever reads.
            let all = lines.clone();
            let indices = visible.clone();
            uniform_list("log", indices.len(), move |range, _window, _cx| {
                range
                    .map(|position| {
                        let line = indices
                            .get(position)
                            .and_then(|index| all.get(*index))
                            .cloned()
                            .unwrap_or_default();
                        div()
                            .w_full()
                            .h(LINE_HEIGHT)
                            .px_3()
                            .text_size(px(12.))
                            .font_family("monospace")
                            .text_color(colors.text_secondary)
                            .child(line)
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        } else if indirect_pods_loading {
            crate::skeleton::detail(cx)
        } else if let Some(error) = indirect_pods_error {
            self.notice(error, true, cx)
        } else if indirect && self.log_pod(cx).is_none() {
            self.notice(
                rust_i18n::t!(match node {
                    true => "detail.node_logs_empty",
                    false => "detail.workload_logs_empty",
                })
                .to_string(),
                false,
                cx,
            )
        } else if let Some(error) = error {
            self.notice(error, true, cx)
        } else if loading {
            crate::skeleton::detail(cx)
        } else if !query.trim().is_empty() && !lines.is_empty() {
            self.notice(rust_i18n::t!("table.no_matches").to_string(), false, cx)
        } else {
            self.notice(rust_i18n::t!("detail.logs_empty").to_string(), false, cx)
        };

        v_flex()
            .size_full()
            .bg(tokens.colors().bg_terminal)
            .child(toolbar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(body),
            )
            .into_any_element()
    }

    /// A small on/off chip in the log's toolbar.
    fn toggle(
        &self,
        id: &'static str,
        label: String,
        on: bool,
        cx: &mut Context<Self>,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let tokens = Tokens::global(cx).clone();
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .flex_shrink_0()
            .rounded(px(tokens.radius.control()))
            .cursor_pointer()
            .text_size(px(11.))
            .when(on, |this| {
                this.bg(tokens.colors().row_active())
                    .text_color(tokens.colors().text_primary)
            })
            .when(!on, |this| this.text_color(tokens.colors().text_muted))
            .hover(|this| this.bg(tokens.colors().row_hover()))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| act(this, cx)))
    }

    /// One muted or red line.
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

impl Render for Detail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = self.object(cx) else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(self.notice(rust_i18n::t!("detail.empty").to_string(), false, cx));
        };
        let header = self.header(&object, cx).into_any_element();
        let tabs = self.tabs(cx).into_any_element();
        let body = match self.tab {
            Tab::Overview => self.overview(&object, cx),
            Tab::Resources => self.resources(&object, cx),
            Tab::Events => self.events(&object, cx),
            Tab::Yaml => self.yaml(&object, window, cx),
            Tab::Logs => self.logs(cx),
            Tab::Run => self.run_tab(&object, cx),
            Tab::Shell => self.shell_tab(&object, window, cx),
        };
        let footer = self.footer(&object, cx);
        v_flex()
            .size_full()
            .child(header)
            .child(tabs)
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(footer)
    }
}

/// Whether this catalogue identity is the Argo CD Application kind.
fn is_argo_application(resource: &kirikumo_kube::ResourceKey) -> bool {
    resource.group == kirikumo_ui::gitops::ARGO_CD_GROUP && resource.kind == "Application"
}

/// Whether a resource is the core Node kind.
fn is_node(resource: &kirikumo_kube::ResourceKey) -> bool {
    resource.group.is_empty() && resource.kind == "Node"
}

/// How one span of the shell's screen is drawn.
///
/// The cursor is the text colour and the ground swapped while the panel has
/// focus, and an outline of it — a dimmer swap — while it does not, so a
/// reader can tell where their keystrokes would go before they go there.
fn shell_run(span: &terminal::Span, colors: &kirikumo_ui::theme::Colors, focused: bool) -> TextRun {
    let Style {
        foreground,
        background,
        bold,
        italic,
        underline,
        cursor,
    } = span.style;
    let mut font = font("monospace");
    if bold {
        font.weight = FontWeight::SEMIBOLD;
    }
    if italic {
        font.style = FontStyle::Italic;
    }
    let (color, background_color) = match cursor {
        true => (
            colors.bg_terminal,
            Some(match focused {
                true => colors.text_primary,
                false => colors.text_muted,
            }),
        ),
        false => (
            foreground
                .map(|colour| colors.terminal(colour))
                .unwrap_or(colors.text_primary),
            background.map(|colour| colors.terminal(colour)),
        ),
    };
    TextRun {
        len: span.text.len(),
        font,
        color,
        background_color,
        underline: underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: false,
        }),
        strikethrough: None,
    }
}
