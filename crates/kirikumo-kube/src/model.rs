//! The generic object model.
//!
//! Kubernetes already has a data model and it is *unstructured*: every object
//! is JSON with `apiVersion`, `kind`, `metadata` and whatever else its schema
//! says. Typing each kind here would mean a code change for every custom
//! resource, which is exactly what this app exists not to need
//! (`AGENTS.md` rule 8). So an [`Object`] is the raw value plus the one
//! schema every kind shares — its [`ObjectMeta`] — and everything drawn from
//! it is a function of that value, written once and tested against real
//! payloads.

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// One entry in the kubeconfig, as the context picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRef {
    /// The context's own name, which is what the reader picks by.
    pub name: String,
    /// The cluster entry it points at.
    pub cluster: String,
    /// The user entry it points at, if it names one.
    pub user: Option<String>,
    /// The namespace it defaults to, if it names one.
    pub namespace: Option<String>,
    /// The cluster's server address, shown under the name so two contexts on
    /// two clusters with the same name can be told apart.
    pub server: String,
    /// Whether this context turns certificate verification off, which the
    /// window says out loud (`docs/ui.md` §3.2).
    pub insecure: bool,
}

/// What the apiserver says it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ClusterVersion {
    /// `"1"`, usually.
    #[serde(default)]
    pub major: String,
    /// `"31"`, or `"31+"` on a managed cluster.
    #[serde(default)]
    pub minor: String,
    /// The whole of it: `"v1.31.2"`.
    #[serde(default, rename = "gitVersion")]
    pub git_version: String,
    /// `"linux/arm64"`.
    #[serde(default)]
    pub platform: String,
}

impl ClusterVersion {
    /// What the sidebar header shows: the git version if there is one, and
    /// the major/minor pair otherwise, because a managed apiserver sometimes
    /// only reports the latter.
    pub fn label(&self) -> String {
        if !self.git_version.is_empty() {
            self.git_version.clone()
        } else if !self.major.is_empty() {
            format!("v{}.{}", self.major, self.minor)
        } else {
            String::new()
        }
    }
}

/// The identity of a kind, stable across versions.
///
/// A kind is keyed by its group and its kind name, never by its *version*:
/// the sidebar's selection and the settings that remember it must survive a
/// cluster upgrade that moves `autoscaling/v2beta2` to `autoscaling/v2`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceKey {
    /// The API group, empty for the core group.
    pub group: String,
    /// The kind, e.g. `Pod`.
    pub kind: String,
}

impl ResourceKey {
    /// A key from its parts.
    pub fn new(group: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            group: group.into(),
            kind: kind.into(),
        }
    }

    /// `Pod` for the core group, `Deployment.apps` otherwise — the same
    /// spelling `kubectl` accepts, and what the settings file stores.
    pub fn qualified(&self) -> String {
        if self.group.is_empty() {
            self.kind.clone()
        } else {
            format!("{}.{}", self.kind, self.group)
        }
    }

    /// Read back what [`Self::qualified`] wrote.
    pub fn parse(qualified: &str) -> Option<Self> {
        let (kind, group) = match qualified.split_once('.') {
            Some((kind, group)) => (kind, group),
            None => (qualified, ""),
        };
        (!kind.is_empty()).then(|| Self::new(group, kind))
    }
}

/// One thing the cluster serves.
///
/// Everything the table and the sidebar need to address a kind: where its
/// collection lives, whether it is namespaced, and what may be done to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResource {
    /// The API group, empty for the core group.
    pub group: String,
    /// The version this app talks to, which is the group's preferred one
    /// unless the cluster only serves another.
    pub version: String,
    /// The kind, e.g. `Pod`.
    pub kind: String,
    /// The plural path segment, e.g. `pods`.
    pub name: String,
    /// The singular name, e.g. `pod`.
    pub singular: String,
    /// Whether objects of this kind live in a namespace.
    pub namespaced: bool,
    /// What the apiserver says may be done: `list`, `watch`, `delete`, …
    pub verbs: Vec<String>,
    /// `po`, `deploy` — what the reader may type to find it.
    pub short_names: Vec<String>,
    /// `all`, and whatever else the cluster groups it under.
    pub categories: Vec<String>,
}

impl ApiResource {
    /// `v1` for the core group, `apps/v1` otherwise.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    /// Where this kind's collection lives: `/api/v1` or `/apis/apps/v1`.
    ///
    /// The core group is the one that is not under `/apis`, which is a
    /// historical accident every Kubernetes client has to encode somewhere.
    pub fn url_prefix(&self) -> String {
        if self.group.is_empty() {
            format!("/api/{}", self.version)
        } else {
            format!("/apis/{}/{}", self.group, self.version)
        }
    }

    /// The path of a collection, optionally scoped to a namespace.
    ///
    /// A namespace is ignored for a cluster-scoped kind rather than being an
    /// error: the table keeps a namespace selected while the reader moves
    /// between kinds, and asking for `/namespaces/x/nodes` would 404.
    pub fn collection_path(&self, namespace: Option<&str>) -> String {
        match namespace.filter(|ns| self.namespaced && !ns.is_empty()) {
            Some(namespace) => format!(
                "{}/namespaces/{}/{}",
                self.url_prefix(),
                namespace,
                self.name
            ),
            None => format!("{}/{}", self.url_prefix(), self.name),
        }
    }

    /// The path of one object.
    pub fn object_path(&self, namespace: Option<&str>, name: &str) -> String {
        format!("{}/{}", self.collection_path(namespace), name)
    }

    /// This kind's stable identity.
    pub fn key(&self) -> ResourceKey {
        ResourceKey::new(self.group.clone(), self.kind.clone())
    }

    /// Whether the apiserver says this verb is available.
    pub fn supports(&self, verb: &str) -> bool {
        self.verbs.iter().any(|known| known == verb)
    }

    /// Whether this kind can be listed at all, which is what decides if it
    /// gets a row in the sidebar. A subresource (`pods/log`) cannot.
    pub fn is_listable(&self) -> bool {
        self.supports("list") && !self.name.contains('/')
    }
}

/// The one schema every object shares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectMeta {
    /// The object's name.
    pub name: String,
    /// Its namespace, for a namespaced kind.
    pub namespace: Option<String>,
    /// The uid, which is how events are found and how a row is keyed across
    /// a delete-and-recreate.
    pub uid: String,
    /// The version a watch resumes from.
    pub resource_version: String,
    /// When it was created, which is what an age column is.
    pub created: Option<DateTime<Utc>>,
    /// When it was asked to go away; set means it is `Terminating`.
    pub deleted: Option<DateTime<Utc>>,
    /// Its labels, ordered so the detail panel is stable between renders.
    pub labels: BTreeMap<String, String>,
    /// Its annotations, likewise.
    pub annotations: BTreeMap<String, String>,
    /// What owns it, which is how a Pod is navigated back to its Deployment.
    pub owners: Vec<OwnerRef>,
}

/// One `metadata.ownerReferences` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerRef {
    /// The owner's kind.
    pub kind: String,
    /// The owner's name.
    pub name: String,
    /// The owner's `apiVersion`, which says which group to look in.
    pub api_version: String,
    /// The owner's uid.
    pub uid: String,
    /// Whether this is the controller, as opposed to a mere owner. Only the
    /// controller is worth a link in the detail panel.
    pub controller: bool,
}

/// One object: what the apiserver sent, and the part of it every kind has.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    /// The object exactly as it arrived. Everything not in [`Self::meta`] is
    /// read from here, per kind, by whoever needs it.
    pub raw: Value,
    /// The parsed `metadata`.
    pub meta: ObjectMeta,
}

impl Object {
    /// Parse an object from what the apiserver sent.
    ///
    /// A missing or malformed `metadata` is an error rather than a default:
    /// a row with no name is a row that cannot be selected, opened or
    /// watched, and one silently drawn as blank is worse than a loud failure.
    pub fn new(raw: Value) -> Result<Self> {
        let meta = ObjectMeta::parse(raw.get("metadata"))?;
        Ok(Self { raw, meta })
    }

    /// The object's `kind`, when it says (a list's items often do not, so a
    /// caller that knows better should not ask).
    pub fn kind(&self) -> Option<&str> {
        self.raw.get("kind").and_then(Value::as_str)
    }

    /// A field by dotted path: `at("status.phase")`.
    ///
    /// The one accessor everything kind-specific is written in terms of. A
    /// path segment that is an integer indexes an array, so
    /// `spec.containers.0.image` works.
    pub fn at(&self, path: &str) -> Option<&Value> {
        let mut cursor = &self.raw;
        for segment in path.split('.') {
            cursor = match cursor {
                Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
                other => other.get(segment)?,
            };
        }
        Some(cursor)
    }

    /// A string field by dotted path, empty when it is absent or not a
    /// string. Callers draw cells with this, and an absent field is a blank
    /// cell rather than a panic.
    pub fn str_at(&self, path: &str) -> &str {
        self.at(path).and_then(Value::as_str).unwrap_or_default()
    }

    /// An integer field by dotted path, `0` when absent.
    ///
    /// Zero is the right default for every counter Kubernetes has —
    /// `readyReplicas`, `restartCount`, `succeeded` — because the apiserver
    /// omits them precisely when they are zero.
    pub fn int_at(&self, path: &str) -> i64 {
        self.at(path).and_then(Value::as_i64).unwrap_or_default()
    }

    /// A boolean field by dotted path, `false` when absent.
    pub fn bool_at(&self, path: &str) -> bool {
        self.at(path).and_then(Value::as_bool).unwrap_or_default()
    }

    /// An array field by dotted path, empty when absent.
    pub fn array_at(&self, path: &str) -> &[Value] {
        self.at(path)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// One entry of `status.conditions`, by type.
    ///
    /// Conditions are the closest thing Kubernetes has to a universal status,
    /// and every kind that has them spells them the same way.
    pub fn condition(&self, kind: &str) -> Option<&Value> {
        self.array_at("status.conditions")
            .iter()
            .find(|condition| condition.get("type").and_then(Value::as_str) == Some(kind))
    }

    /// Whether a condition is `"True"`.
    ///
    /// Kubernetes conditions are tri-state strings, not booleans: `Unknown`
    /// means the controller has lost touch, and treating it as `false` is
    /// how a viewer reports a partitioned node as merely not ready.
    pub fn condition_is(&self, kind: &str, status: &str) -> bool {
        self.condition(kind)
            .and_then(|condition| condition.get("status"))
            .and_then(Value::as_str)
            == Some(status)
    }
}

impl ObjectMeta {
    /// Parse a `metadata` block.
    fn parse(value: Option<&Value>) -> Result<Self> {
        let value = value.ok_or_else(|| Error::Malformed("an object with no metadata".into()))?;
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| value.get("generateName").and_then(Value::as_str))
            .ok_or_else(|| Error::Malformed("an object with no name".into()))?
            .to_string();
        Ok(Self {
            name,
            namespace: value
                .get("namespace")
                .and_then(Value::as_str)
                .filter(|namespace| !namespace.is_empty())
                .map(str::to_string),
            uid: value
                .get("uid")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            resource_version: value
                .get("resourceVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            created: parse_time(value.get("creationTimestamp")),
            deleted: parse_time(value.get("deletionTimestamp")),
            labels: parse_map(value.get("labels")),
            annotations: parse_map(value.get("annotations")),
            owners: value
                .get("ownerReferences")
                .and_then(Value::as_array)
                .map(|owners| owners.iter().filter_map(OwnerRef::parse).collect())
                .unwrap_or_default(),
        })
    }

    /// Whether the object is on its way out.
    pub fn is_terminating(&self) -> bool {
        self.deleted.is_some()
    }

    /// What identifies this object among others of its kind.
    ///
    /// The uid, because a deleted-and-recreated object with the same name is
    /// a different object: it must not inherit the old one's selection, and a
    /// watch must not treat its `ADDED` as a modification. Falling back to
    /// `namespace/name` covers the answers that omit the uid, which some
    /// aggregated apiservers do.
    ///
    /// One function, because the table keys its rows by this and the watch
    /// matches its events by it; two spellings of "the same object" is how a
    /// live table grows duplicates.
    pub fn identity(&self) -> String {
        if !self.uid.is_empty() {
            return self.uid.clone();
        }
        match &self.namespace {
            Some(namespace) => format!("{namespace}/{}", self.name),
            None => self.name.clone(),
        }
    }

    /// The controller that made this object, if one did.
    pub fn controller(&self) -> Option<&OwnerRef> {
        self.owners.iter().find(|owner| owner.controller)
    }
}

impl OwnerRef {
    fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            kind: value.get("kind")?.as_str()?.to_string(),
            name: value.get("name")?.as_str()?.to_string(),
            api_version: value
                .get("apiVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            uid: value
                .get("uid")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            controller: value
                .get("controller")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        })
    }

    /// The group of the owner's `apiVersion`, so the owner can be looked up
    /// in the catalogue by [`ResourceKey`].
    pub fn group(&self) -> &str {
        match self.api_version.split_once('/') {
            Some((group, _)) => group,
            None => "",
        }
    }

    /// The owner's identity in the catalogue.
    pub fn key(&self) -> ResourceKey {
        ResourceKey::new(self.group(), self.kind.clone())
    }
}

/// A page of objects, and where a watch on them starts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectList {
    /// The objects.
    pub items: Vec<Object>,
    /// The list's own `resourceVersion`, which is the point a watch resumes
    /// from so that nothing between the list and the watch is missed
    /// (roadmap §4.7).
    pub resource_version: String,
    /// The continue token, when the apiserver paged us.
    pub next: Option<String>,
}

impl ObjectList {
    /// Parse a `…List` answer.
    ///
    /// An item that cannot be parsed is dropped with a warning rather than
    /// failing the list: one malformed object in a namespace of four thousand
    /// must not blank the table.
    pub fn parse(value: Value) -> Result<Self> {
        let resource_version = value
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let next = value
            .pointer("/metadata/continue")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        let items = match value.get("items") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| match Object::new(item.clone()) {
                    Ok(object) => Some(object),
                    Err(error) => {
                        tracing::warn!(%error, "skipping an object that could not be parsed");
                        None
                    }
                })
                .collect(),
            _ => return Err(Error::Malformed("a list with no items".into())),
        };
        Ok(Self {
            items,
            resource_version,
            next,
        })
    }
}

/// The sidebar's groups: what a kind is filed under (`docs/ui.md` §3.2).
///
/// The order is Lens's, because it is the order people already know. It is a
/// *presentation* of the catalogue, not a filter on it: everything the
/// cluster serves appears somewhere, and anything these groups do not name
/// falls into [`Group::Custom`], which is what makes a custom resource free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// Nodes, Namespaces, Events and the rest of the cluster's own objects.
    Cluster,
    /// Pods and the controllers that make them.
    Workloads,
    /// ConfigMaps, Secrets, quotas, autoscalers.
    Config,
    /// Services, Ingresses, policies.
    Network,
    /// Claims, volumes, classes.
    Storage,
    /// Service accounts, roles, bindings.
    AccessControl,
    /// Argo CD applications, application sets and projects.
    GitOps,
    /// Everything else, under its API group.
    Custom,
}

impl Group {
    /// Every group, in the order the sidebar draws them.
    pub const ALL: &'static [Group] = &[
        Group::Cluster,
        Group::Workloads,
        Group::Config,
        Group::Network,
        Group::Storage,
        Group::AccessControl,
        Group::GitOps,
        Group::Custom,
    ];

    /// The locale key for the group's heading.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Cluster => "group.cluster",
            Self::Workloads => "group.workloads",
            Self::Config => "group.config",
            Self::Network => "group.network",
            Self::Storage => "group.storage",
            Self::AccessControl => "group.access_control",
            Self::GitOps => "group.gitops",
            Self::Custom => "group.custom",
        }
    }
}

/// Every kind a cluster serves, sorted into the sidebar's groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalogue {
    /// Every listable resource, one preferred version per kind, in the order
    /// the sidebar draws them.
    pub resources: Vec<ApiResource>,
}

impl Catalogue {
    /// A catalogue over a set of resources, already deduplicated and sorted
    /// by [`crate::discovery`].
    pub fn new(resources: Vec<ApiResource>) -> Self {
        Self { resources }
    }

    /// The resource for a kind, if the cluster serves it.
    pub fn get(&self, key: &ResourceKey) -> Option<&ApiResource> {
        self.resources
            .iter()
            .find(|resource| &resource.key() == key)
    }

    /// Whether the cluster serves a kind at all, which is how a detail panel
    /// decides if an owner reference is worth a link.
    pub fn has(&self, key: &ResourceKey) -> bool {
        self.get(key).is_some()
    }

    /// The resources in one of the sidebar's groups, in order.
    pub fn in_group(&self, group: Group) -> Vec<&ApiResource> {
        self.resources
            .iter()
            .filter(|resource| crate::discovery::group_of(resource) == group)
            .collect()
    }
}

/// What to fetch a log for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRequest {
    /// The pod's namespace.
    pub namespace: String,
    /// The pod's name.
    pub pod: String,
    /// Which container, when the pod has more than one.
    pub container: Option<String>,
    /// How many lines from the end. `None` is everything, which on a chatty
    /// pod is megabytes, so callers should say a number.
    pub tail_lines: Option<u32>,
    /// The previous instance's log, which is the only place a crash loop's
    /// reason survives.
    pub previous: bool,
    /// Whether each line carries the time the kubelet saw it.
    pub timestamps: bool,
    /// Whether to keep the connection open and read what comes next.
    pub follow: bool,
}

impl LogRequest {
    /// A request for the tail of a pod's log.
    pub fn new(namespace: impl Into<String>, pod: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            pod: pod.into(),
            container: None,
            // The default `kubectl logs` has none, and a pod that has been up
            // for a week would arrive as a hundred megabytes of text this
            // window would then have to lay out.
            tail_lines: Some(2000),
            previous: false,
            timestamps: false,
            follow: false,
        }
    }

    /// Ask for one container's log.
    pub fn container(mut self, container: impl Into<String>) -> Self {
        self.container = Some(container.into());
        self
    }

    /// Ask for the previous instance's log.
    pub fn previous(mut self, previous: bool) -> Self {
        self.previous = previous;
        self
    }

    /// Ask to keep reading as the container writes.
    pub fn follow(mut self, follow: bool) -> Self {
        self.follow = follow;
        self
    }

    /// The query string this request becomes.
    pub fn query(&self) -> String {
        let mut parts = Vec::new();
        if let Some(container) = &self.container {
            parts.push(format!("container={container}"));
        }
        if let Some(lines) = self.tail_lines {
            parts.push(format!("tailLines={lines}"));
        }
        if self.previous {
            parts.push("previous=true".to_string());
        }
        if self.timestamps {
            parts.push("timestamps=true".to_string());
        }
        if self.follow {
            parts.push("follow=true".to_string());
        }
        parts.join("&")
    }
}

/// One `v1.Event`, as the detail panel's Events tab shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    /// `Normal` or `Warning`.
    pub kind: String,
    /// `Scheduled`, `BackOff`, `FailedMount`.
    pub reason: String,
    /// What happened, in the controller's own words.
    pub message: String,
    /// Which component said it.
    pub source: String,
    /// How many times it has happened.
    pub count: i64,
    /// The last time it did.
    pub last: Option<DateTime<Utc>>,
}

impl EventRecord {
    /// Read one from an event object.
    ///
    /// Kubernetes has two event schemas — the core one and `events.k8s.io` —
    /// and a cluster answers with whichever the caller asked for. The fields
    /// that differ (`lastTimestamp` against `deprecatedLastTimestamp`,
    /// `count` against `deprecatedCount`) are both read, so the same code
    /// works against either.
    pub fn parse(object: &Object) -> Self {
        let last = parse_time(object.at("lastTimestamp"))
            .or_else(|| parse_time(object.at("deprecatedLastTimestamp")))
            .or_else(|| parse_time(object.at("eventTime")))
            .or(object.meta.created);
        let count = match object.int_at("count") {
            0 => object.int_at("deprecatedCount").max(1),
            counted => counted,
        };
        let source = match object.str_at("source.component") {
            "" => object.str_at("reportingComponent").to_string(),
            component => component.to_string(),
        };
        Self {
            kind: match object.str_at("type") {
                "" => "Normal".to_string(),
                kind => kind.to_string(),
            },
            reason: object.str_at("reason").to_string(),
            message: object.str_at("message").to_string(),
            source,
            count,
            last,
        }
    }

    /// Whether this event is one the reader should be worried about.
    pub fn is_warning(&self) -> bool {
        self.kind == "Warning"
    }
}

/// What one node or pod is using right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metrics {
    /// The node's or pod's name.
    pub name: String,
    /// The pod's namespace; `None` for a node.
    pub namespace: Option<String>,
    /// CPU in milli-cores, the unit Kubernetes quotes requests in.
    pub cpu_milli: u64,
    /// Memory in bytes.
    pub memory_bytes: u64,
}

/// A change to send to the apiserver.
///
/// Three patch types, because Kubernetes has three and they are not
/// interchangeable: a strategic merge is what `kubectl` uses on built-in
/// kinds and is the only one that merges a list of containers correctly, a
/// plain merge is all a custom resource supports, and JSON Patch is the only
/// one that can remove a key unambiguously.
#[derive(Debug, Clone, PartialEq)]
pub enum Patch {
    /// `application/strategic-merge-patch+json`.
    Strategic(Value),
    /// `application/merge-patch+json`.
    Merge(Value),
    /// `application/json-patch+json`; the value must be an array of ops.
    Json(Value),
    /// A whole object, sent as a `PUT`. What applying an edited YAML is.
    Replace(Value),
}

impl Patch {
    /// The `Content-Type` this patch is sent with.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Strategic(_) => "application/strategic-merge-patch+json",
            Self::Merge(_) => "application/merge-patch+json",
            Self::Json(_) => "application/json-patch+json",
            Self::Replace(_) => "application/json",
        }
    }

    /// The body.
    pub fn body(&self) -> &Value {
        match self {
            Self::Strategic(value)
            | Self::Merge(value)
            | Self::Json(value)
            | Self::Replace(value) => value,
        }
    }
}

/// Parse an RFC 3339 timestamp, which is the only shape Kubernetes uses.
fn parse_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let text = value?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

/// Parse a `map[string]string`, dropping anything that is not one.
fn parse_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod() -> Object {
        Object::new(json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "api-7d9f8c-2xk",
                "namespace": "default",
                "uid": "1f3c",
                "resourceVersion": "48210",
                "creationTimestamp": "2026-09-04T10:11:12Z",
                "labels": {"app": "api"},
                "ownerReferences": [{
                    "apiVersion": "apps/v1",
                    "kind": "ReplicaSet",
                    "name": "api-7d9f8c",
                    "uid": "aa11",
                    "controller": true
                }]
            },
            "spec": {"containers": [{"name": "api", "image": "ghcr.io/x/api:1.4"}]},
            "status": {
                "phase": "Running",
                "conditions": [{"type": "Ready", "status": "True"}]
            }
        }))
        .unwrap()
    }

    #[test]
    fn metadata_is_the_only_thing_typed() {
        let pod = pod();
        assert_eq!(pod.meta.name, "api-7d9f8c-2xk");
        assert_eq!(pod.meta.namespace.as_deref(), Some("default"));
        assert_eq!(pod.meta.resource_version, "48210");
        assert_eq!(pod.meta.labels.get("app").map(String::as_str), Some("api"));
        assert!(pod.meta.created.is_some());
        assert!(!pod.meta.is_terminating());
        assert_eq!(
            pod.meta.controller().map(|o| o.name.as_str()),
            Some("api-7d9f8c")
        );
        assert_eq!(
            pod.meta.controller().unwrap().key(),
            ResourceKey::new("apps", "ReplicaSet")
        );
    }

    #[test]
    fn a_dotted_path_reaches_into_arrays() {
        let pod = pod();
        assert_eq!(pod.str_at("spec.containers.0.image"), "ghcr.io/x/api:1.4");
        assert_eq!(pod.str_at("spec.containers.9.image"), "");
        assert_eq!(pod.str_at("status.phase"), "Running");
        assert_eq!(pod.int_at("status.nothing"), 0);
    }

    #[test]
    fn a_condition_is_tri_state_and_unknown_is_not_false() {
        let pod = pod();
        assert!(pod.condition_is("Ready", "True"));
        assert!(!pod.condition_is("Ready", "False"));
        assert!(!pod.condition_is("Missing", "True"));
    }

    #[test]
    fn an_object_with_no_name_is_an_error_rather_than_a_blank_row() {
        assert!(Object::new(json!({"metadata": {}})).is_err());
        assert!(Object::new(json!({"kind": "Pod"})).is_err());
    }

    #[test]
    fn a_list_keeps_the_version_a_watch_resumes_from() {
        let list = ObjectList::parse(json!({
            "kind": "PodList",
            "metadata": {"resourceVersion": "48211"},
            "items": [pod().raw]
        }))
        .unwrap();
        assert_eq!(list.resource_version, "48211");
        assert_eq!(list.items.len(), 1);
        assert!(list.next.is_none());
    }

    #[test]
    fn one_unreadable_item_does_not_blank_the_table() {
        let list = ObjectList::parse(json!({
            "metadata": {"resourceVersion": "1"},
            "items": [pod().raw, json!({"metadata": {}})]
        }))
        .unwrap();
        assert_eq!(list.items.len(), 1);
    }

    #[test]
    fn the_core_group_is_the_one_that_is_not_under_apis() {
        let pods = ApiResource {
            group: String::new(),
            version: "v1".into(),
            kind: "Pod".into(),
            name: "pods".into(),
            singular: "pod".into(),
            namespaced: true,
            verbs: vec!["list".into(), "watch".into()],
            short_names: vec!["po".into()],
            categories: vec!["all".into()],
        };
        assert_eq!(pods.api_version(), "v1");
        assert_eq!(
            pods.collection_path(Some("kube-system")),
            "/api/v1/namespaces/kube-system/pods"
        );
        assert_eq!(pods.collection_path(None), "/api/v1/pods");
        assert_eq!(
            pods.object_path(Some("default"), "api"),
            "/api/v1/namespaces/default/pods/api"
        );

        let deployments = ApiResource {
            group: "apps".into(),
            version: "v1".into(),
            kind: "Deployment".into(),
            name: "deployments".into(),
            singular: "deployment".into(),
            namespaced: true,
            verbs: vec!["list".into()],
            short_names: vec!["deploy".into()],
            categories: vec![],
        };
        assert_eq!(deployments.api_version(), "apps/v1");
        assert_eq!(
            deployments.collection_path(None),
            "/apis/apps/v1/deployments"
        );
    }

    #[test]
    fn a_namespace_is_ignored_by_a_cluster_scoped_kind_rather_than_being_an_error() {
        let nodes = ApiResource {
            group: String::new(),
            version: "v1".into(),
            kind: "Node".into(),
            name: "nodes".into(),
            singular: "node".into(),
            namespaced: false,
            verbs: vec!["list".into()],
            short_names: vec![],
            categories: vec![],
        };
        assert_eq!(nodes.collection_path(Some("default")), "/api/v1/nodes");
    }

    #[test]
    fn a_subresource_never_gets_a_row() {
        let logs = ApiResource {
            group: String::new(),
            version: "v1".into(),
            kind: "Pod".into(),
            name: "pods/log".into(),
            singular: String::new(),
            namespaced: true,
            verbs: vec!["get".into()],
            short_names: vec![],
            categories: vec![],
        };
        assert!(!logs.is_listable());
    }

    #[test]
    fn a_key_round_trips_through_the_spelling_kubectl_uses() {
        for key in [
            ResourceKey::new("", "Pod"),
            ResourceKey::new("apps", "Deployment"),
        ] {
            assert_eq!(ResourceKey::parse(&key.qualified()), Some(key));
        }
        assert_eq!(ResourceKey::new("", "Pod").qualified(), "Pod");
        assert_eq!(
            ResourceKey::new("apps", "Deployment").qualified(),
            "Deployment.apps"
        );
        assert!(ResourceKey::parse("").is_none());
    }

    #[test]
    fn a_log_request_asks_for_a_tail_by_default() {
        let request = LogRequest::new("default", "api")
            .container("proxy")
            .previous(true);
        let query = request.query();
        assert!(query.contains("container=proxy"));
        assert!(query.contains("tailLines=2000"));
        assert!(query.contains("previous=true"));
        // Not following unless asked: the plain tail is what a panel opens on.
        assert!(!query.contains("follow"));
        assert!(
            LogRequest::new("default", "api")
                .follow(true)
                .query()
                .contains("follow=true")
        );
    }

    #[test]
    fn an_event_reads_either_schema() {
        let deprecated = Object::new(json!({
            "metadata": {"name": "e1", "creationTimestamp": "2026-09-05T00:00:00Z"},
            "type": "Warning",
            "reason": "BackOff",
            "message": "Back-off restarting failed container",
            "deprecatedCount": 7,
            "deprecatedLastTimestamp": "2026-09-06T00:00:00Z",
            "reportingComponent": "kubelet"
        }))
        .unwrap();
        let event = EventRecord::parse(&deprecated);
        assert!(event.is_warning());
        assert_eq!(event.count, 7);
        assert_eq!(event.source, "kubelet");
        assert_eq!(
            event.last.unwrap().to_rfc3339(),
            "2026-09-06T00:00:00+00:00"
        );
    }

    #[test]
    fn an_event_with_no_type_is_normal() {
        let object = Object::new(json!({"metadata": {"name": "e"}, "reason": "Pulled"})).unwrap();
        let event = EventRecord::parse(&object);
        assert_eq!(event.kind, "Normal");
        assert_eq!(event.count, 1);
    }

    #[test]
    fn each_patch_says_how_it_must_be_sent() {
        assert_eq!(
            Patch::Strategic(json!({})).content_type(),
            "application/strategic-merge-patch+json"
        );
        assert_eq!(
            Patch::Merge(json!({})).content_type(),
            "application/merge-patch+json"
        );
        assert_eq!(
            Patch::Json(json!([])).content_type(),
            "application/json-patch+json"
        );
        assert_eq!(Patch::Replace(json!({})).content_type(), "application/json");
    }

    #[test]
    fn a_version_falls_back_to_major_and_minor() {
        let version = ClusterVersion {
            major: "1".into(),
            minor: "31+".into(),
            ..ClusterVersion::default()
        };
        assert_eq!(version.label(), "v1.31+");
        assert_eq!(ClusterVersion::default().label(), "");
    }
}
