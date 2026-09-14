//! Everything fetched, and the fetching of it.
//!
//! One entity holds every answer the cluster has given this window, each as a
//! [`Fetch`] so a refresh keeps the old value on screen. Every call goes to
//! the background executor and comes back through `this.update`; nothing here
//! blocks the UI thread, and nothing but this file calls the trait
//! (`AGENTS.md` rule 2).
//!
//! Nothing is written to disk (rule 10). Unlike this app's siblings there is
//! no snapshot and no HTTP cache: Kubernetes payloads are large, short-lived
//! and frequently secret, and the apiserver has no `ETag` to make a cheap
//! revalidation out of.

use gpui::{AppContext as _, Context, EventEmitter};
use kirikumo_kube::portforward::{POLL, Poll};
use kirikumo_kube::{
    ApiResource, Applied, Catalogue, Cluster, ClusterVersion, ContextRef, EventRecord, ExecOutput,
    ExecRequest, Forwarder, LogRequest, Metrics, Object, ObjectList, Patch, PrinterColumn,
    PrinterColumns, ResourceKey, WatchEvent, crd, drain, exec, logs, watch,
};
use kirikumo_ui::Fetch;
use kirikumo_ui::fetch::describe;
use kirikumo_ui::terminal::Screen;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

/// Which list: a kind, scoped to a namespace or to all of them.
pub type ListKey = (ResourceKey, Option<String>);

/// Which object: a kind, a namespace and a name.
pub type ObjectKey = (ResourceKey, Option<String>, String);

/// A write, ready to send.
///
/// Built by the detail panel from `kirikumo_kube::actions` and carried here
/// whole, so the store knows only the narrow operation the trait exposes and
/// not how the button that requested it was presented.
#[derive(Debug, Clone)]
pub enum Write {
    /// Remove the object.
    Delete,
    /// Change it.
    Patch(Patch),
    /// Create one Job from this CronJob's template.
    TriggerCronJob(Object),
    /// Cordon the node and move everything off it that can move.
    Drain,
}

/// Emitted whenever an answer lands.
pub enum StoreEvent {
    /// Something changed; views re-read what they show.
    Changed,
}

/// The window's memory of the cluster.
pub struct Store {
    cluster: Arc<dyn Cluster>,
    /// The contexts the kubeconfig offers, and which one is connected.
    contexts: Vec<ContextRef>,
    current: Option<String>,
    /// Whether the connected context skips certificate verification, which
    /// the sidebar says out loud (`docs/ui.md` §3.2).
    insecure: bool,
    version: Fetch<ClusterVersion>,
    catalogue: Fetch<Catalogue>,
    namespaces: Fetch<Vec<String>>,
    /// The columns each custom kind declares, read from the CRDs once
    /// discovery has said the cluster has any.
    printer_columns: Fetch<PrinterColumns>,
    lists: HashMap<ListKey, Fetch<ObjectList>>,
    events: HashMap<String, Fetch<Vec<EventRecord>>>,
    /// Each container's log, as lines — never as one string, because the
    /// view is a virtualized list over them and splitting a fifty-thousand
    /// line log on every frame is the whole performance budget.
    logs: HashMap<String, Fetch<Vec<String>>>,
    /// The log being followed, if one is.
    log_follow: Option<LogFollow>,
    /// Bumped for every follow started, so a line from an abandoned one is
    /// recognised and dropped.
    log_generation: u64,
    /// What every node is using, when the cluster has a metrics server.
    node_metrics: Fetch<Vec<Metrics>>,
    /// What every pod in a namespace is using, likewise, by namespace.
    pod_metrics: HashMap<Option<String>, Fetch<Vec<Metrics>>>,
    /// Whether this login may do a verb on a kind in a namespace, per
    /// `SelfSubjectAccessReview`. Asked once per triple and kept: RBAC does
    /// not change under a window often enough to be worth asking again.
    permissions: HashMap<(ResourceKey, Option<String>, String), Fetch<bool>>,
    /// The last write on each object: in flight, done, or refused. A write
    /// that landed carries a line to show — empty for most, and a drain's
    /// report for a drain.
    writes: HashMap<ObjectKey, Fetch<String>>,
    /// The last command run in each pod: in flight, its output, or refused.
    runs: HashMap<ObjectKey, Fetch<ExecOutput>>,
    /// Whether this login may exec into pods, per namespace: the
    /// `pods/exec` subresource, which the catalogue does not list.
    exec_permissions: HashMap<Option<String>, Fetch<bool>>,
    /// Every local port being forwarded to a pod. Dropped with the store, or
    /// on a change of context: a forward is a hole into one cluster.
    forwards: Vec<ActiveForward>,
    /// The shell attached to a pod, if one is. One at a time: the panel
    /// shows one pod, and a shell nobody can see is a thread and a socket
    /// for nothing.
    shell: Option<ShellSession>,
    /// Bumped for every shell attached, so bytes from one that has been
    /// detached are recognised and dropped.
    shell_generation: u64,
    /// The list the window is looking at, and so the only one worth
    /// following. A cluster with ten thousand pods must not be streamed
    /// because the sidebar mentions pods (roadmap §4.7).
    followed: Option<ListKey>,
    /// The watch that is running, if one is.
    watch: Option<WatchHandle>,
    /// Bumped for every watch started, so an event from one that has been
    /// abandoned is recognised and dropped.
    generation: u64,
}

/// A local port being forwarded to a pod.
pub struct ActiveForward {
    /// Which pod.
    pub pod: ObjectKey,
    /// The port on the pod.
    pub remote: u16,
    /// The listener, which stops when this is dropped.
    pub forwarder: Forwarder,
}

impl ActiveForward {
    /// The port on `localhost`.
    pub fn local(&self) -> u16 {
        self.forwarder.local_port()
    }
}

/// Where an attached shell is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellStatus {
    /// The websocket is being opened.
    Connecting,
    /// Bytes are flowing.
    Open,
    /// The shell exited, or the pod closed the connection.
    Closed,
    /// It could not be opened, or the connection broke: why.
    Failed(String),
}

/// A shell attached to a pod: its screen, and the way to reach it.
///
/// The tunnel itself lives on the pump thread, which is the only thing that
/// touches it; what is typed and the size go to it over a channel. The
/// screen lives here, on the UI thread, fed from what the pump sends back.
pub struct ShellSession {
    /// Which pod.
    pub pod: ObjectKey,
    /// Which container, when one was named.
    pub container: Option<String>,
    /// What the shell has drawn.
    pub screen: Screen,
    /// Where it is.
    pub status: ShellStatus,
    /// Which generation it belongs to.
    generation: u64,
    /// Keystrokes and resizes, for the pump to pass on.
    input: mpsc::Sender<ShellInput>,
    /// Set when it is abandoned; the pump notices at its next poll.
    stop: Arc<AtomicBool>,
}

/// What the UI thread sends a shell's pump.
enum ShellInput {
    /// Bytes to type.
    Bytes(Vec<u8>),
    /// The panel is `cols` by `rows` now.
    Resize(u16, u16),
}

/// What a shell's pump sends back.
enum ShellOutput {
    /// Bytes the shell printed.
    Bytes(Vec<u8>),
    /// The tunnel is open and bytes may be typed.
    Opened,
    /// The pod closed it.
    Closed,
    /// It could not be opened or it broke: why.
    Failed(String),
}

/// A log being followed, and the way to tell it to stop.
struct LogFollow {
    /// Which generation it belongs to.
    generation: u64,
    /// Set when it is abandoned. Noticed at the next line the container
    /// writes, or when the channel closes.
    stop: Arc<AtomicBool>,
}

/// A watch that is running, and the way to tell it to stop.
struct WatchHandle {
    /// Which list it follows.
    key: ListKey,
    /// Which generation it belongs to.
    generation: u64,
    /// Set when it is abandoned.
    ///
    /// The reader is a blocking read on a thread of its own and cannot be
    /// interrupted from here, so it notices at its next event or at the
    /// apiserver's timeout — five minutes at the outside
    /// (`kirikumo_kube::rest`). Until then it is a parked thread and a socket,
    /// and its events are dropped by generation.
    stop: Arc<AtomicBool>,
}

impl EventEmitter<StoreEvent> for Store {}

impl Store {
    /// A store over a cluster. Nothing is fetched until asked.
    pub fn new(cluster: Arc<dyn Cluster>) -> Self {
        Self {
            cluster,
            contexts: Vec::new(),
            current: None,
            insecure: false,
            version: Fetch::Idle,
            catalogue: Fetch::Idle,
            namespaces: Fetch::Idle,
            printer_columns: Fetch::Idle,
            lists: HashMap::new(),
            events: HashMap::new(),
            logs: HashMap::new(),
            log_follow: None,
            log_generation: 0,
            node_metrics: Fetch::Idle,
            pod_metrics: HashMap::new(),
            permissions: HashMap::new(),
            writes: HashMap::new(),
            runs: HashMap::new(),
            exec_permissions: HashMap::new(),
            forwards: Vec::new(),
            shell: None,
            shell_generation: 0,
            followed: None,
            watch: None,
            generation: 0,
        }
    }

    /// Tell the store which contexts exist and which one it is connected to.
    pub fn with_contexts(mut self, contexts: Vec<ContextRef>, current: Option<String>) -> Self {
        self.insecure = contexts
            .iter()
            .find(|context| Some(&context.name) == current.as_ref())
            .is_some_and(|context| context.insecure);
        self.contexts = contexts;
        self.current = current;
        self
    }

    /// The contexts the kubeconfig offers.
    pub fn contexts(&self) -> &[ContextRef] {
        &self.contexts
    }

    /// The context this store is connected to.
    pub fn current_context(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// The server of the connected context, for the insecure warning.
    pub fn current_server(&self) -> Option<&str> {
        self.contexts
            .iter()
            .find(|context| Some(&context.name) == self.current.as_ref())
            .map(|context| context.server.as_str())
    }

    /// Whether certificate verification is off on this connection.
    pub fn is_insecure(&self) -> bool {
        self.insecure
    }

    /// Point the store at another cluster, forgetting everything the last one
    /// said.
    ///
    /// Everything: a list of pods from one cluster drawn under another
    /// cluster's name is the single worst thing a viewer like this can do.
    pub fn connect(
        &mut self,
        cluster: Arc<dyn Cluster>,
        context: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.cluster = cluster;
        self.insecure = self
            .contexts
            .iter()
            .find(|entry| Some(&entry.name) == context.as_ref())
            .is_some_and(|entry| entry.insecure);
        self.current = context;
        self.version = Fetch::Idle;
        self.catalogue = Fetch::Idle;
        self.namespaces = Fetch::Idle;
        self.printer_columns = Fetch::Idle;
        self.lists.clear();
        self.events.clear();
        self.stop_following_log();
        self.logs.clear();
        self.node_metrics = Fetch::Idle;
        self.pod_metrics.clear();
        self.permissions.clear();
        self.writes.clear();
        self.runs.clear();
        self.exec_permissions.clear();
        // Dropping a forwarder stops it: nothing from the last cluster may
        // stay reachable on `localhost` under the new one's name.
        self.forwards.clear();
        self.detach_shell();
        self.stop_watch();
        self.followed = None;
        cx.emit(StoreEvent::Changed);
        cx.notify();
        self.refresh_all(cx);
    }

    /// What the apiserver says it is.
    pub fn version(&self) -> &Fetch<ClusterVersion> {
        &self.version
    }

    /// Everything the cluster serves.
    pub fn catalogue(&self) -> &Fetch<Catalogue> {
        &self.catalogue
    }

    /// Every namespace.
    pub fn namespaces(&self) -> &Fetch<Vec<String>> {
        &self.namespaces
    }

    /// The resource for a kind, if discovery has landed and the cluster
    /// serves it.
    pub fn resource(&self, key: &ResourceKey) -> Option<&ApiResource> {
        self.catalogue
            .value()
            .and_then(|catalogue| catalogue.get(key))
    }

    /// The key a list is stored under.
    ///
    /// A cluster-scoped kind is stored once, whatever namespace is selected,
    /// so moving between namespaces does not re-fetch the nodes.
    pub fn list_key(&self, key: &ResourceKey, namespace: Option<&str>) -> ListKey {
        let namespaced = self
            .resource(key)
            .is_some_and(|resource| resource.namespaced);
        let namespace = namespace
            .filter(|namespace| namespaced && !namespace.is_empty())
            .map(str::to_string);
        (key.clone(), namespace)
    }

    /// A list, if it has ever been asked for.
    pub fn list(&self, key: &ResourceKey, namespace: Option<&str>) -> Option<&Fetch<ObjectList>> {
        self.lists.get(&self.list_key(key, namespace))
    }

    /// Fetch a list only if it never has been.
    pub fn ensure_list(
        &mut self,
        key: ResourceKey,
        namespace: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let stored = self.list_key(&key, namespace);
        if self.lists.get(&stored).is_none_or(Fetch::is_idle) {
            self.load_list(key, namespace, cx);
        }
    }

    /// Fetch a list.
    pub fn load_list(&mut self, key: ResourceKey, namespace: Option<&str>, cx: &mut Context<Self>) {
        let Some(resource) = self.resource(&key).cloned() else {
            // Discovery has not landed, or this cluster does not serve the
            // kind the settings remembered. Either way there is nothing to
            // ask for; the sidebar will not have drawn a row for it.
            return;
        };
        let stored = self.list_key(&key, namespace);
        self.lists.entry(stored.clone()).or_default().begin();
        let scope = stored.1.clone();
        self.fetch(
            cx,
            move |cluster| cluster.list(&resource, scope.as_deref()),
            move |this, result, cx| {
                let landed = result.is_ok();
                this.lists.entry(stored.clone()).or_default().finish(result);
                // A watch resumes from the version the list came back with,
                // so it can only start once there is a list.
                if landed && this.followed.as_ref() == Some(&stored) {
                    this.start_watch(cx);
                }
            },
        );
    }

    /// Follow one list, and stop following whatever was followed before.
    ///
    /// One watch at a time, because one table is on screen at a time. The
    /// watch starts when a list for this key has landed — it resumes from
    /// that list's `resourceVersion`, so there is nothing to resume from
    /// before then.
    pub fn follow(&mut self, key: Option<ListKey>, cx: &mut Context<Self>) {
        if self.followed == key {
            return;
        }
        self.stop_watch();
        self.followed = key;
        self.start_watch(cx);
    }

    /// Whether the list on screen is being followed rather than refreshed.
    pub fn is_live(&self) -> bool {
        self.watch.is_some()
    }

    /// Start a watch on the followed list, if there is one to start.
    fn start_watch(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.followed.clone() else {
            return;
        };
        if self.watch.as_ref().is_some_and(|watch| watch.key == key) {
            return;
        }
        let Some(resource) = self.resource(&key.0).cloned() else {
            return;
        };
        // A cluster may serve a kind it will not let anyone follow, and RBAC
        // may allow `list` and refuse `watch`. Either way the table stays
        // correct; it is refreshed rather than live.
        if !resource.supports("watch") {
            tracing::debug!(kind = %resource.kind, "no watch for this kind");
            return;
        }
        let Some(version) = self
            .lists
            .get(&key)
            .and_then(Fetch::value)
            .map(|list| list.resource_version.clone())
            .filter(|version| !version.is_empty())
        else {
            return;
        };

        self.stop_watch();
        self.generation += 1;
        let generation = self.generation;
        let stop = Arc::new(AtomicBool::new(false));
        self.watch = Some(WatchHandle {
            key: key.clone(),
            generation,
            stop: stop.clone(),
        });

        let (sender, receiver) = async_channel::unbounded::<WatchEvent>();
        let cluster = self.cluster.clone();
        let namespace = key.1.clone();
        let kind = resource.kind.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("kirikumo-watch-{kind}"))
            .spawn(move || {
                watch::pump(cluster, resource, namespace, version, stop, |event| {
                    sender.send_blocking(event).is_ok()
                })
            })
        {
            tracing::warn!(%error, %kind, "could not start a watch");
            self.watch = None;
            return;
        }

        cx.spawn(async move |this, cx| {
            while let Ok(event) = receiver.recv().await {
                let carry_on = this
                    .update(cx, |this, cx| this.on_watch(generation, event, cx))
                    .unwrap_or(false);
                if !carry_on {
                    break;
                }
            }
        })
        .detach();
    }

    /// Tell the running watch to stop, and stop believing it.
    fn stop_watch(&mut self) {
        if let Some(watch) = self.watch.take() {
            watch.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Apply one watch event, and say whether the watch should carry on.
    fn on_watch(&mut self, generation: u64, event: WatchEvent, cx: &mut Context<Self>) -> bool {
        // An event from a watch we have abandoned: its thread has not
        // noticed yet, and its list may not even be here any more.
        let Some(key) = self
            .watch
            .as_ref()
            .filter(|watch| watch.generation == generation)
            .map(|watch| watch.key.clone())
        else {
            return false;
        };
        let Some(list) = self.lists.get_mut(&key).and_then(Fetch::value_mut) else {
            return false;
        };
        match watch::apply(list, event) {
            applied @ (Applied::Added(_) | Applied::Changed(_) | Applied::Removed(_)) => {
                // At `trace` rather than `debug`: one line per object per
                // event is the right grain for diagnosing a table that will
                // not settle, and far too much for anything else.
                tracing::trace!(?applied, kind = %key.0.kind, "a watch event landed");
                cx.emit(StoreEvent::Changed);
                cx.notify();
                true
            }
            // A bookmark: nothing on screen moved, and the version it left
            // behind is the thread's business, not ours.
            Applied::Version => true,
            Applied::Restart => {
                // The version we were resuming from has aged out of the
                // apiserver's window. List again; that lands a new version
                // and starts a new watch.
                tracing::debug!("the watch aged out; listing again");
                self.stop_watch();
                self.load_list(key.0, key.1.as_deref(), cx);
                false
            }
            Applied::Failed(error) => {
                // Logged, not shown: the table is still correct, it has just
                // stopped being live, and blanking a good list over it would
                // be worse than the loss.
                tracing::warn!(%error, "the watch stopped");
                self.stop_watch();
                false
            }
        }
    }

    /// One object out of a list that has already landed.
    ///
    /// The detail panel reads from the list rather than fetching again: the
    /// object it wants arrived a moment ago, and a `GET` for it would show
    /// the reader a spinner over data the window already has.
    pub fn object(&self, key: &ObjectKey) -> Option<&Object> {
        let (kind, namespace, name) = key;
        self.lists
            .iter()
            .filter(|((stored, _), _)| stored == kind)
            .filter_map(|(_, list)| list.value())
            .flat_map(|list| list.items.iter())
            .find(|object| {
                &object.meta.name == name
                    && (namespace.is_none() || object.meta.namespace == *namespace)
            })
    }

    /// The events about an object, if they have ever been asked for.
    pub fn events(&self, uid: &str) -> Option<&Fetch<Vec<EventRecord>>> {
        self.events.get(uid)
    }

    /// Fetch an object's events only if they never have been.
    pub fn ensure_events(
        &mut self,
        uid: String,
        namespace: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if uid.is_empty() || self.events.get(&uid).is_some_and(|fetch| !fetch.is_idle()) {
            return;
        }
        self.load_events(uid, namespace, cx);
    }

    /// Fetch an object's events again, including after an empty result.
    pub fn load_events(&mut self, uid: String, namespace: Option<String>, cx: &mut Context<Self>) {
        if uid.is_empty() {
            return;
        }
        self.events.entry(uid.clone()).or_default().begin();
        let key = uid.clone();
        self.fetch(
            cx,
            move |cluster| cluster.events_for(&uid, namespace.as_deref()),
            move |this, result, _| {
                this.events.entry(key).or_default().finish(result);
            },
        );
    }

    /// A container's log, if it has ever been asked for.
    pub fn logs(&self, key: &str) -> Option<&Fetch<Vec<String>>> {
        self.logs.get(key)
    }

    /// Whether a log is being followed.
    pub fn is_following_log(&self) -> bool {
        self.log_follow.is_some()
    }

    /// The key a log is stored under: the pod, and the container within it.
    pub fn log_key(request: &LogRequest) -> String {
        format!(
            "{}/{}:{}",
            request.namespace,
            request.pod,
            request.container.as_deref().unwrap_or_default()
        )
    }

    /// Show a container's log, following it or not.
    ///
    /// The two are one request, not two: `follow=true&tailLines=n` replays
    /// the tail and then keeps going, so following is not "fetch, then also
    /// stream" — it is the same question asked with the connection left open.
    pub fn show_log(&mut self, request: LogRequest, follow: bool, cx: &mut Context<Self>) {
        self.stop_following_log();
        let key = Self::log_key(&request);
        match follow {
            true => self.start_following_log(key, request, cx),
            false => {
                if self.logs.get(&key).is_none_or(Fetch::is_idle) {
                    self.load_logs(request, cx);
                }
            }
        }
    }

    /// Fetch a log's tail, once.
    pub fn load_logs(&mut self, request: LogRequest, cx: &mut Context<Self>) {
        let key = Self::log_key(&request);
        self.logs.entry(key.clone()).or_default().begin();
        self.fetch(
            cx,
            move |cluster| cluster.logs(&request),
            move |this, result, _| {
                let lines = result.map(|text| text.lines().map(str::to_string).collect());
                this.logs.entry(key).or_default().finish(lines);
            },
        );
    }

    /// Follow a log: read it on a thread and append what arrives.
    fn start_following_log(&mut self, key: String, request: LogRequest, cx: &mut Context<Self>) {
        // A followed log starts empty and fills, rather than showing the last
        // tail while the new one arrives: the lines are about to be replayed
        // anyway, and showing them twice is worse than showing them late.
        self.logs
            .insert(key.clone(), Fetch::Loading { stale: None });
        self.log_generation += 1;
        let generation = self.log_generation;
        let stop = Arc::new(AtomicBool::new(false));
        self.log_follow = Some(LogFollow {
            generation,
            stop: stop.clone(),
        });

        let (sender, receiver) = async_channel::unbounded::<Result<String, String>>();
        let cluster = self.cluster.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("kirikumo-log-{}", request.pod))
            .spawn(move || pump_log(cluster, request, stop, sender))
        {
            tracing::warn!(%error, "could not start following a log");
            self.log_follow = None;
            return;
        }

        cx.spawn(async move |this, cx| {
            while let Ok(message) = receiver.recv().await {
                let carry_on = this
                    .update(cx, |this, cx| {
                        this.on_log_line(generation, &key, message, cx)
                    })
                    .unwrap_or(false);
                if !carry_on {
                    break;
                }
            }
        })
        .detach();
    }

    /// Stop following, if anything is.
    pub fn stop_following_log(&mut self) {
        if let Some(follow) = self.log_follow.take() {
            follow.stop.store(true, Ordering::Relaxed);
        }
    }

    /// One line from a followed log, and whether to keep reading.
    fn on_log_line(
        &mut self,
        generation: u64,
        key: &str,
        message: Result<String, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .log_follow
            .as_ref()
            .is_none_or(|follow| follow.generation != generation)
        {
            return false;
        }
        let held = self.logs.entry(key.to_string()).or_default();
        match message {
            Ok(line) => {
                // The first line turns a `Loading` into a `Ready`, so the
                // skeleton gives way the moment the container says anything.
                if held.value().is_none() {
                    held.finish(Ok(Vec::new()));
                }
                if let Some(lines) = held.value_mut() {
                    logs::append(lines, line);
                }
                cx.emit(StoreEvent::Changed);
                cx.notify();
                true
            }
            Err(error) => {
                held.finish(Err(error));
                self.stop_following_log();
                cx.emit(StoreEvent::Changed);
                cx.notify();
                false
            }
        }
    }

    /// Ask for what a kind's objects are using, if it is a kind that uses
    /// anything and the cluster can say.
    ///
    /// `metrics.k8s.io` is an optional add-on. A cluster without it answers
    /// with a failure, which is kept as one and never retried on its own: the
    /// detail panel simply draws no *Using* section, which is the truthful
    /// thing to draw when nobody can say.
    pub fn ensure_metrics(&mut self, kind: &str, namespace: Option<&str>, cx: &mut Context<Self>) {
        match kind {
            "Node" => {
                if !self.node_metrics.is_idle() {
                    return;
                }
                self.node_metrics.begin();
                self.fetch(
                    cx,
                    |cluster| cluster.node_metrics(),
                    |this, result, _| this.node_metrics.finish(result),
                );
            }
            "Pod" => {
                let scope = namespace.map(str::to_string);
                if self
                    .pod_metrics
                    .get(&scope)
                    .is_some_and(|fetch| !fetch.is_idle())
                {
                    return;
                }
                self.pod_metrics.entry(scope.clone()).or_default().begin();
                let asked = scope.clone();
                self.fetch(
                    cx,
                    move |cluster| cluster.pod_metrics(asked.as_deref()),
                    move |this, result, _| {
                        this.pod_metrics.entry(scope).or_default().finish(result);
                    },
                );
            }
            _ => {}
        }
    }

    /// Refresh metrics that have previously succeeded, keeping their stale
    /// values on screen while the next sample is fetched.
    ///
    /// An unsupported metrics API is deliberately not retried on this clock;
    /// only a successful first answer opts a scope into periodic sampling.
    pub fn refresh_metrics_if_available(
        &mut self,
        kind: &str,
        namespace: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        match kind {
            "Node" if self.node_metrics.value().is_some() && !self.node_metrics.is_loading() => {
                self.node_metrics.begin();
                self.fetch(
                    cx,
                    |cluster| cluster.node_metrics(),
                    |this, result, _| this.node_metrics.finish(result),
                );
            }
            "Pod" => {
                let scope = namespace.map(str::to_string);
                let refresh = self
                    .pod_metrics
                    .get(&scope)
                    .is_some_and(|fetch| fetch.value().is_some() && !fetch.is_loading());
                if !refresh {
                    return;
                }
                self.pod_metrics.entry(scope.clone()).or_default().begin();
                let asked = scope.clone();
                self.fetch(
                    cx,
                    move |cluster| cluster.pod_metrics(asked.as_deref()),
                    move |this, result, _| {
                        this.pod_metrics.entry(scope).or_default().finish(result);
                    },
                );
            }
            _ => {}
        }
    }

    /// What one object is using, if anything has said.
    pub fn metrics_for(&self, kind: &str, namespace: Option<&str>, name: &str) -> Option<&Metrics> {
        self.metrics(kind, namespace)?
            .iter()
            .find(|metrics| metrics.name == name)
    }

    /// Every metric in one table scope after the optional API answered.
    ///
    /// `Some(&[])` means the metrics API exists but the scope is empty;
    /// `None` also covers loading and unsupported clusters. That distinction
    /// lets the table add usage columns only when the cluster can populate
    /// them.
    pub fn metrics(&self, kind: &str, namespace: Option<&str>) -> Option<&[Metrics]> {
        let held = match kind {
            "Node" => self.node_metrics.value()?,
            "Pod" => self
                .pod_metrics
                .get(&namespace.map(str::to_string))?
                .value()?,
            _ => return None,
        };
        Some(held.as_slice())
    }

    /// Whether this login may do a verb on a kind, if the cluster has said.
    ///
    /// `None` while it is being asked, which the panel draws the same way as
    /// `false`: a button that lights up when the answer is yes is better than
    /// one that is pressable for a second and then is not.
    pub fn permission(
        &self,
        key: &ResourceKey,
        namespace: Option<&str>,
        verb: &str,
    ) -> Option<bool> {
        self.permissions
            .get(&(key.clone(), namespace.map(str::to_string), verb.to_string()))
            .and_then(|fetch| fetch.value())
            .copied()
    }

    /// Ask whether this login may do a verb on a kind, once.
    pub fn ensure_permission(
        &mut self,
        key: ResourceKey,
        namespace: Option<String>,
        verb: &'static str,
        cx: &mut Context<Self>,
    ) {
        let stored = (key.clone(), namespace.clone(), verb.to_string());
        if self
            .permissions
            .get(&stored)
            .is_some_and(|fetch| !fetch.is_idle())
        {
            return;
        }
        let Some(resource) = self.resource(&key).cloned() else {
            return;
        };
        self.permissions.entry(stored.clone()).or_default().begin();
        self.fetch(
            cx,
            move |cluster| cluster.can_i(&resource, namespace.as_deref(), verb),
            move |this, result, _| {
                // A cluster that cannot be asked — an aggregated apiserver
                // with no authorization endpoint — is taken as a yes, and the
                // write itself is left to be the judge.
                let answer = result.or_else(|error| {
                    tracing::debug!(%error, "could not ask about permissions; assuming yes");
                    Ok::<bool, String>(true)
                });
                this.permissions.entry(stored).or_default().finish(answer);
            },
        );
    }

    /// The last write on an object, if there has been one.
    pub fn write(&self, object: &ObjectKey) -> Option<&Fetch<String>> {
        self.writes.get(object)
    }

    /// Send a write, and when it lands, list the kind again.
    ///
    /// Listing again rather than patching what is held: the answer to a write
    /// is one object, and what the reader is looking at is the list, whose
    /// other rows the write may have moved (a scale makes pods). A watch
    /// would bring the change anyway; the re-list is for a cluster without
    /// one, and costs one request.
    pub fn perform(&mut self, object: ObjectKey, write: Write, cx: &mut Context<Self>) {
        let (key, namespace, name) = object.clone();
        let Some(resource) = self.resource(&key).cloned() else {
            return;
        };
        self.writes.entry(object.clone()).or_default().begin();
        let scope = namespace;
        // A drain also needs to know where pods live, which only the
        // catalogue can say.
        let pods = self.resource(&drain::pods_key()).cloned();
        let drained = matches!(write, Write::Drain);
        let triggered = matches!(write, Write::TriggerCronJob(_));
        self.fetch(
            cx,
            move |cluster| match write {
                Write::Delete => cluster
                    .delete(&resource, scope.as_deref(), &name)
                    .map(|()| String::new()),
                Write::Patch(patch) => cluster
                    .patch(&resource, scope.as_deref(), &name, patch)
                    .map(|_| String::new()),
                Write::TriggerCronJob(cron_job) => cluster
                    .trigger_cron_job(&resource, &cron_job)
                    .map(|job| rust_i18n::t!("action.triggered", name = job.meta.name).to_string()),
                Write::Drain => {
                    let pods = pods.ok_or(kirikumo_kube::Error::Unsupported)?;
                    // Slow on purpose when a budget resists; this is the
                    // background executor, and the footer says *Working…*.
                    drain::drain(cluster, &resource, &pods, &name, std::thread::sleep)
                        .map(|report| report.summary())
                }
            },
            move |this, result, cx| {
                let landed = result.is_ok();
                this.writes.entry(object).or_default().finish(result);
                if landed {
                    // Every list of the kind, not just the one scoped like
                    // the object: the table may be showing all namespaces
                    // while the object was opened in one of them. A drain
                    // moved pods as well as touching the node, so their
                    // lists are asked for too.
                    let pods = drain::pods_key();
                    let jobs = ResourceKey::new("batch", "Job");
                    let held: Vec<ListKey> = this
                        .lists
                        .keys()
                        .filter(|(kind, _)| {
                            *kind == key
                                || (drained && *kind == pods)
                                || (triggered && *kind == jobs)
                        })
                        .cloned()
                        .collect();
                    for (kind, scope) in held {
                        this.load_list(kind, scope.as_deref(), cx);
                    }
                }
            },
        );
    }

    /// The last command run in a pod, if one has been.
    pub fn run(&self, pod: &ObjectKey) -> Option<&Fetch<ExecOutput>> {
        self.runs.get(pod)
    }

    /// Run a command in a pod and keep what it said.
    ///
    /// Only one at a time per pod: a second run replaces the first's output
    /// when it lands, and the panel says *Working…* meanwhile.
    pub fn exec(&mut self, pod: ObjectKey, request: ExecRequest, cx: &mut Context<Self>) {
        self.runs.entry(pod.clone()).or_default().begin();
        self.fetch(
            cx,
            move |cluster| cluster.exec(&request),
            move |this, result, _| {
                this.runs.entry(pod).or_default().finish(result);
            },
        );
    }

    /// Whether this login may exec into pods in a namespace, if the cluster
    /// has said.
    pub fn exec_permission(&self, namespace: Option<&str>) -> Option<bool> {
        self.exec_permissions
            .get(&namespace.map(str::to_string))
            .and_then(|fetch| fetch.value())
            .copied()
    }

    /// Ask whether this login may exec into pods in a namespace, once.
    pub fn ensure_exec_permission(&mut self, namespace: Option<String>, cx: &mut Context<Self>) {
        if self
            .exec_permissions
            .get(&namespace)
            .is_some_and(|fetch| !fetch.is_idle())
        {
            return;
        }
        let Some(pods) = self.resource(&drain::pods_key()).cloned() else {
            return;
        };
        let review = exec::review_resource(&pods);
        self.exec_permissions
            .entry(namespace.clone())
            .or_default()
            .begin();
        let scope = namespace.clone();
        self.fetch(
            cx,
            move |cluster| cluster.can_i(&review, scope.as_deref(), "create"),
            move |this, result, _| {
                let answer = result.or_else(|error| {
                    tracing::debug!(%error, "could not ask about exec; assuming yes");
                    Ok::<bool, String>(true)
                });
                this.exec_permissions
                    .entry(namespace)
                    .or_default()
                    .finish(answer);
            },
        );
    }

    /// The forwards open to one pod.
    pub fn forwards_for<'a>(
        &'a self,
        pod: &'a ObjectKey,
    ) -> impl Iterator<Item = &'a ActiveForward> {
        self.forwards
            .iter()
            .filter(move |forward| &forward.pod == pod)
    }

    /// How many forwards are open, to any pod.
    pub fn forward_count(&self) -> usize {
        self.forwards.len()
    }

    /// Forward a local port to a port on a pod.
    ///
    /// The same number is tried on `localhost` first, because `8080 → 8080`
    /// is what a person expects; when it is taken, any free port is used and
    /// the answer says which. Binding is immediate; the tunnel to the
    /// apiserver is opened per connection, on that connection's thread, so
    /// this returns before anything has been sent to the cluster.
    pub fn start_forward(
        &mut self,
        pod: ObjectKey,
        remote: u16,
        cx: &mut Context<Self>,
    ) -> Result<u16, String> {
        if let Some(existing) = self
            .forwards
            .iter()
            .find(|forward| forward.pod == pod && forward.remote == remote)
        {
            return Ok(existing.local());
        }
        let (_, namespace, name) = pod.clone();
        let namespace = namespace.unwrap_or_default();
        let cluster = self.cluster.clone();
        let connect = move || cluster.port_forward(&namespace, &name, remote);
        let forwarder = match Forwarder::serve(remote, connect) {
            Ok(forwarder) => forwarder,
            Err(_) => {
                // The same closure again, for any free port.
                let (_, namespace, name) = pod.clone();
                let namespace = namespace.unwrap_or_default();
                let cluster = self.cluster.clone();
                Forwarder::serve(0, move || cluster.port_forward(&namespace, &name, remote))
                    .map_err(|error| describe(&error))?
            }
        };
        let local = forwarder.local_port();
        tracing::info!(local, remote, pod = %pod.2, "forwarding");
        self.forwards.push(ActiveForward {
            pod,
            remote,
            forwarder,
        });
        cx.emit(StoreEvent::Changed);
        cx.notify();
        Ok(local)
    }

    /// Stop forwarding to a port on a pod.
    pub fn stop_forward(&mut self, pod: &ObjectKey, remote: u16, cx: &mut Context<Self>) {
        self.forwards
            .retain(|forward| !(&forward.pod == pod && forward.remote == remote));
        cx.emit(StoreEvent::Changed);
        cx.notify();
    }

    /// The shell attached to a pod, if one is.
    pub fn shell(&self) -> Option<&ShellSession> {
        self.shell.as_ref()
    }

    /// Attach a shell to a pod, `cols` by `rows`.
    ///
    /// Any shell already attached — to this pod or another — is detached
    /// first. The websocket is opened on a thread of its own, which then
    /// pumps: what is typed goes down it between polls, what comes back is
    /// fed to the screen here.
    pub fn attach_shell(
        &mut self,
        pod: ObjectKey,
        request: ExecRequest,
        cols: u16,
        rows: u16,
        cx: &mut Context<Self>,
    ) {
        self.detach_shell();
        self.shell_generation += 1;
        let generation = self.shell_generation;
        let stop = Arc::new(AtomicBool::new(false));
        let (input, inbox) = mpsc::channel::<ShellInput>();
        let (sender, receiver) = async_channel::unbounded::<ShellOutput>();
        let container = request.container.clone();
        let cluster = self.cluster.clone();
        let started = std::thread::Builder::new()
            .name(format!("kirikumo-shell-{}", request.pod))
            .spawn({
                let stop = stop.clone();
                move || pump_shell(cluster, request, (cols, rows), inbox, stop, sender)
            });
        if let Err(error) = started {
            tracing::warn!(%error, "could not start a shell");
            return;
        }
        self.shell = Some(ShellSession {
            pod,
            container,
            screen: Screen::new(cols, rows),
            status: ShellStatus::Connecting,
            generation,
            input,
            stop,
        });
        cx.spawn(async move |this, cx| {
            while let Ok(message) = receiver.recv().await {
                let carry_on = this
                    .update(cx, |this, cx| this.on_shell_output(generation, message, cx))
                    .unwrap_or(false);
                if !carry_on {
                    break;
                }
            }
        })
        .detach();
        cx.emit(StoreEvent::Changed);
        cx.notify();
    }

    /// Type into the attached shell.
    pub fn type_into_shell(&mut self, bytes: Vec<u8>) {
        if let Some(shell) = &self.shell
            && shell.status == ShellStatus::Open
        {
            let _ = shell.input.send(ShellInput::Bytes(bytes));
        }
    }

    /// Tell the attached shell the panel is `cols` by `rows` now.
    ///
    /// The screen is resized here at once and the shell told afterwards, so
    /// the two agree by the time the shell redraws.
    pub fn resize_shell(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        if let Some(shell) = &mut self.shell
            && shell.screen.resize(cols, rows)
        {
            let _ = shell.input.send(ShellInput::Resize(cols, rows));
            cx.notify();
        }
    }

    /// Detach the shell, if one is attached.
    pub fn detach_shell(&mut self) {
        if let Some(shell) = self.shell.take() {
            shell.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Something from the shell's pump, and whether to keep listening.
    fn on_shell_output(
        &mut self,
        generation: u64,
        message: ShellOutput,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(shell) = self
            .shell
            .as_mut()
            .filter(|shell| shell.generation == generation)
        else {
            return false;
        };
        let carry_on = match message {
            ShellOutput::Opened => {
                shell.status = ShellStatus::Open;
                true
            }
            ShellOutput::Bytes(bytes) => {
                shell.screen.feed(&bytes);
                true
            }
            ShellOutput::Closed => {
                shell.status = ShellStatus::Closed;
                false
            }
            ShellOutput::Failed(why) => {
                shell.status = ShellStatus::Failed(why);
                false
            }
        };
        cx.emit(StoreEvent::Changed);
        cx.notify();
        carry_on
    }

    /// Ask the cluster who it is, what it serves and what namespaces it has.
    ///
    /// The three questions every other question depends on, asked once per
    /// connection.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.version.begin();
        self.fetch(
            cx,
            |cluster| cluster.version(),
            |this, result, _| this.version.finish(result),
        );
        self.catalogue.begin();
        self.fetch(
            cx,
            |cluster| cluster.catalogue(),
            |this, result, cx| {
                this.catalogue.finish(result);
                // What the custom kinds' tables look like is in their CRDs,
                // which can only be asked for once discovery has said the
                // cluster serves CRDs at all.
                this.load_printer_columns(cx);
            },
        );
        self.namespaces.begin();
        self.fetch(
            cx,
            |cluster| cluster.namespaces(),
            |this, result, _| this.namespaces.finish(result),
        );
    }

    /// The columns a custom kind declares, once its CRD has been read.
    pub fn printer_columns(&self, key: &ResourceKey) -> Option<&[PrinterColumn]> {
        self.printer_columns.value()?.get(key).map(Vec::as_slice)
    }

    /// Read every CRD, for the columns each declares.
    ///
    /// One list, once per connection. A cluster that serves no CRDs — or
    /// will not let this login list them — leaves the map empty, and every
    /// custom kind gets the plain table, which is what it would have had
    /// anyway.
    fn load_printer_columns(&mut self, cx: &mut Context<Self>) {
        let crds = ResourceKey::new("apiextensions.k8s.io", "CustomResourceDefinition");
        let Some(resource) = self.resource(&crds).cloned() else {
            self.printer_columns.finish(Ok(PrinterColumns::new()));
            return;
        };
        self.printer_columns.begin();
        self.fetch(
            cx,
            move |cluster| {
                cluster
                    .list(&resource, None)
                    .map(|list| crd::from_list(&list.items))
            },
            |this, result, _| {
                let columns = result.unwrap_or_else(|error| {
                    tracing::debug!(%error, "could not read the CRDs; custom kinds get the plain table");
                    PrinterColumns::new()
                });
                this.printer_columns.finish(Ok(columns));
            },
        );
    }

    /// Fetch every list that is already on screen again.
    pub fn refresh_lists(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<ListKey> = self.lists.keys().cloned().collect();
        for (key, namespace) in keys {
            self.load_list(key, namespace.as_deref(), cx);
        }
    }

    /// Run one call on the background executor and apply its answer here.
    ///
    /// The whole of the threading model: the trait is blocking, the executor
    /// is GPUI's, and the answer lands back on the UI thread through
    /// `this.update`. No reactor appears anywhere (`AGENTS.md` rule 3).
    fn fetch<T, W, A>(&self, cx: &mut Context<Self>, work: W, apply: A)
    where
        T: Send + 'static,
        W: FnOnce(&dyn Cluster) -> kirikumo_kube::Result<T> + Send + 'static,
        A: FnOnce(&mut Self, Result<T, String>, &mut Context<Self>) + 'static,
    {
        cx.notify();
        let cluster = self.cluster.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    work(cluster.as_ref()).map_err(|error| {
                        // A kind the cluster does not serve — `metrics.k8s.io`
                        // on a cluster with no metrics server, most often —
                        // is an answer, not a failure worth a warning.
                        match error {
                            kirikumo_kube::Error::NotFound(_)
                            | kirikumo_kube::Error::Unsupported => {
                                tracing::debug!(%error, "the cluster does not have that")
                            }
                            _ => tracing::warn!(%error, "a request to the cluster failed"),
                        }
                        describe(&error)
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                apply(this, result, cx);
                cx.emit(StoreEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// One log, read to its end on a thread of its own.
///
/// Simpler than a watch's pump because there is nothing to resume from and
/// nothing to reconnect for: a log that ends has ended, because the container
/// writing it has. A caller that wants it again asks again.
fn pump_log(
    cluster: Arc<dyn Cluster>,
    request: LogRequest,
    stop: Arc<AtomicBool>,
    sender: async_channel::Sender<Result<String, String>>,
) {
    let mut stream = match cluster.follow_logs(&request) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = sender.send_blocking(Err(describe(&error)));
            return;
        }
    };
    while let Some(line) = stream.next_line() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let message = line.map_err(|error| describe(&error));
        let failed = message.is_err();
        if sender.send_blocking(message).is_err() || failed {
            return;
        }
    }
}

/// One shell, opened and then pumped on a thread of its own.
///
/// The thread owns the tunnel outright — nothing else touches it, so there
/// is no lock for a keystroke to wait on. Each turn sends whatever has been
/// typed, then polls for at most `portforward::POLL`; a shell that says
/// nothing costs that much per turn and nothing else. Returns when told to
/// stop, when the pod closes the shell, or when the window has gone.
fn pump_shell(
    cluster: Arc<dyn Cluster>,
    request: ExecRequest,
    (cols, rows): (u16, u16),
    inbox: mpsc::Receiver<ShellInput>,
    stop: Arc<AtomicBool>,
    sender: async_channel::Sender<ShellOutput>,
) {
    let mut tunnel = match cluster.attach(&request) {
        Ok(tunnel) => tunnel,
        Err(error) => {
            let _ = sender.send_blocking(ShellOutput::Failed(describe(&error)));
            return;
        }
    };
    // The size the panel had when it asked, before the shell draws its
    // first prompt at the default eighty by twenty-four.
    let _ = tunnel.resize(cols, rows);
    if sender.send_blocking(ShellOutput::Opened).is_err() {
        tunnel.close();
        return;
    }
    loop {
        if stop.load(Ordering::Relaxed) {
            tunnel.close();
            return;
        }
        let sent = (|| -> kirikumo_kube::Result<()> {
            while let Ok(input) = inbox.try_recv() {
                match input {
                    ShellInput::Bytes(bytes) => tunnel.send(&bytes)?,
                    ShellInput::Resize(cols, rows) => tunnel.resize(cols, rows)?,
                }
            }
            Ok(())
        })();
        let turn = std::time::Instant::now();
        let outcome = match sent.and_then(|()| tunnel.poll()) {
            Ok(Poll::Data(bytes)) => sender
                .send_blocking(ShellOutput::Bytes(bytes))
                .map(|()| true),
            Ok(Poll::Nothing) => {
                // A tunnel that answers "nothing" at once — the scripted
                // one — would otherwise spin this thread; the real one has
                // already waited its turn on the socket.
                if let Some(rest) = POLL.checked_sub(turn.elapsed()) {
                    std::thread::sleep(rest);
                }
                Ok(true)
            }
            Ok(Poll::Closed) => sender.send_blocking(ShellOutput::Closed).map(|()| false),
            Err(error) => sender
                .send_blocking(ShellOutput::Failed(describe(&error)))
                .map(|()| false),
        };
        if !matches!(outcome, Ok(true)) {
            tunnel.close();
            return;
        }
    }
}
