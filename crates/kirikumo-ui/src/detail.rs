//! What the right panel says about one object.
//!
//! The Overview tab, as data: a list of sections, each a list of labelled
//! facts. Written per kind against the JSON, like everything else here, and
//! with a fallback that works for a kind nobody wrote a section for — the
//! metadata every object has, plus its conditions, which is `kubectl
//! describe` reduced to what fits in a 420 px column.

use chrono::{DateTime, Utc};
use kirikumo_kube::{Level, Metrics, Object, ResourceKey, quantity};
use serde_json::Value;

/// Somewhere a fact points.
///
/// The two moves a person makes in a cluster viewer: *up*, from a pod to the
/// thing that made it, and *down*, from a controller to what it made. Up is a
/// single object and is exact. Down is a question — "which pods does this
/// select?" — and is answered the way a person would answer it, by putting
/// the controller's own selector in the filter box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// One object: list its kind and open it.
    Object {
        /// Which kind.
        key: ResourceKey,
        /// Its namespace, or `None` for a cluster-scoped kind.
        namespace: Option<String>,
        /// Its name.
        name: String,
    },
    /// A kind, filtered: list it and put this in the filter box.
    Filtered {
        /// Which kind.
        key: ResourceKey,
        /// Which namespace to scope to.
        namespace: Option<String>,
        /// What to type into the filter for the reader.
        query: String,
    },
}

/// Whether this resource can meaningfully have related Kubernetes Events.
///
/// An Event is already the record of something happening to another object;
/// Kubernetes does not normally emit Events whose subject is another Event.
/// Hiding that recursive tab keeps the cluster-wide Events list distinct from
/// an object's related-events tab.
pub fn has_related_events(resource: &ResourceKey) -> bool {
    !(resource.kind == "Event" && (resource.group.is_empty() || resource.group == "events.k8s.io"))
}

/// Pods whose labels satisfy a workload's selector, newest first.
///
/// Only objects with both a pod template and a non-empty label selector are
/// treated as workloads. This keeps a Service or an empty selector from
/// accidentally turning a Logs tab into a namespace-wide log picker.
pub fn workload_log_pods<'a>(workload: &Object, pods: &'a [Object]) -> Vec<&'a Object> {
    let Some(selector) = workload_log_selector(workload) else {
        return Vec::new();
    };
    let mut match_labels = selector
        .get("matchLabels")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(fields) = selector.as_object() {
        match_labels.extend(
            fields
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "matchLabels" | "matchExpressions"))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let expressions = selector
        .get("matchExpressions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if match_labels.is_empty() && expressions.is_empty() {
        return Vec::new();
    }

    let mut matched: Vec<&Object> = pods
        .iter()
        .filter(|pod| pod.meta.namespace == workload.meta.namespace)
        .filter(|pod| {
            match_labels.iter().all(|(key, value)| {
                value.as_str().is_some_and(|value| {
                    pod.meta.labels.get(key).is_some_and(|label| label == value)
                })
            }) && expressions
                .iter()
                .all(|expression| selector_expression_matches(expression, &pod.meta.labels))
        })
        .collect();
    matched.sort_by(|left, right| {
        right
            .meta
            .created
            .cmp(&left.meta.created)
            .then_with(|| left.meta.name.cmp(&right.meta.name))
    });
    matched
}

/// Whether an object can resolve container logs through selected Pods.
pub fn has_workload_logs(workload: &Object) -> bool {
    workload_log_selector(workload).is_some()
}

fn workload_log_selector(workload: &Object) -> Option<&Value> {
    workload.at("spec.template.spec.containers")?;
    let selector = workload.at("spec.selector")?;
    let has_labels = selector
        .get("matchLabels")
        .and_then(Value::as_object)
        .is_some_and(|labels| !labels.is_empty());
    let has_expressions = selector
        .get("matchExpressions")
        .and_then(Value::as_array)
        .is_some_and(|expressions| !expressions.is_empty());
    let has_legacy_labels = selector.as_object().is_some_and(|fields| {
        fields
            .keys()
            .any(|key| !matches!(key.as_str(), "matchLabels" | "matchExpressions"))
    });
    (has_labels || has_expressions || has_legacy_labels).then_some(selector)
}

fn selector_expression_matches(
    expression: &Value,
    labels: &std::collections::BTreeMap<String, String>,
) -> bool {
    let Some(key) = expression.get("key").and_then(Value::as_str) else {
        return false;
    };
    let Some(operator) = expression.get("operator").and_then(Value::as_str) else {
        return false;
    };
    let values: Vec<&str> = expression
        .get("values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    match operator {
        "In" => labels
            .get(key)
            .is_some_and(|label| values.contains(&label.as_str())),
        "NotIn" => labels
            .get(key)
            .is_none_or(|label| !values.contains(&label.as_str())),
        "Exists" => labels.contains_key(key),
        "DoesNotExist" => !labels.contains_key(key),
        _ => false,
    }
}

/// One labelled fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// The label, already localised or already a Kubernetes field name.
    pub label: String,
    /// The value, formatted for reading.
    pub value: String,
    /// Whether the value is an identifier, and so drawn in the mono family.
    pub mono: bool,
    /// Where this fact goes when it is clicked, if it goes anywhere.
    pub link: Option<Target>,
}

impl Fact {
    /// A fact whose value is prose.
    pub fn text(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            mono: false,
            link: None,
        }
    }

    /// A fact whose value is an identifier: a name, an image, an address.
    pub fn id(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            mono: true,
            link: None,
        }
    }

    /// Make it go somewhere.
    pub fn to(mut self, target: Target) -> Self {
        self.link = Some(target);
        self
    }
}

/// A group of facts under a heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading, or `None` for the first block, which needs none.
    pub title: Option<String>,
    /// The facts.
    pub facts: Vec<Fact>,
}

/// One `status.conditions` entry, as the panel draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    /// `Ready`, `Available`, `PodScheduled`.
    pub kind: String,
    /// `True`, `False`, `Unknown`.
    pub status: String,
    /// Why, when the controller says.
    pub reason: String,
    /// The mark to draw beside it.
    pub level: Level,
}

/// Everything the Overview tab draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    /// The sections, in order.
    pub sections: Vec<Section>,
    /// The conditions, drawn as their own table under the sections.
    pub conditions: Vec<Condition>,
}

/// Build the Overview for an object.
///
/// `usage` is what `metrics.k8s.io` says this object is using, when the
/// cluster has it installed and the object is the kind that has any. `None`
/// means the section is not drawn at all — never a row of zeroes, because a
/// pod using no CPU and a cluster with no metrics server look identical that
/// way and mean opposite things.
pub fn overview(
    kind: &str,
    object: &Object,
    usage: Option<&Metrics>,
    now: DateTime<Utc>,
) -> Overview {
    overview_for(&ResourceKey::new("", kind), object, usage, now)
}

/// Build the Overview when the resource's API group is known.
///
/// The group matters for integrations whose kinds have ordinary names: only
/// `Application.argoproj.io` is an Argo CD application. Keeping this decision
/// here avoids teaching the GPUI view about the custom resource's schema.
pub fn overview_for(
    resource: &ResourceKey,
    object: &Object,
    usage: Option<&Metrics>,
    now: DateTime<Utc>,
) -> Overview {
    let kind = resource.kind.as_str();
    let mut sections = vec![metadata(object, now)];
    if let Some(usage) = usage {
        sections.push(self::usage(kind, object, usage));
    }
    match kind {
        "Pod" => sections.extend(pod(object)),
        "Node" => sections.extend(node(object)),
        "Deployment" | "StatefulSet" | "ReplicaSet" | "DaemonSet" | "Job" => {
            sections.extend(controller(object))
        }
        "Service" => sections.extend(service(object)),
        "PersistentVolumeClaim" => sections.extend(claim(object)),
        "ConfigMap" | "Secret" => sections.extend(keys(object)),
        _ => {}
    }
    if resource.group == crate::gitops::ARGO_CD_GROUP && kind == "Application" {
        sections.extend(application(object));
    }
    if resource.group == crate::gitops::ARGO_CD_GROUP && kind == "ApplicationSet" {
        sections.extend(application_set(object));
    }
    if resource.group == crate::gitops::ARGO_CD_GROUP && kind == "AppProject" {
        sections.extend(app_project(object));
    }
    Overview {
        sections,
        conditions: conditions(object),
    }
}

/// What Argo CD has reconciled for an Application, read from the CR itself.
fn application(object: &Object) -> Vec<Section> {
    let application = crate::gitops::Application::from_object(object);
    let mut status = Vec::new();
    push_text(&mut status, "Project", &application.project);
    push_text(&mut status, "Sync", &application.sync);
    push_text(&mut status, "Health", &application.health);
    push_text(&mut status, "Operation", &application.operation);
    push_id(&mut status, "Revision", &application.revision);
    push_id(&mut status, "Destination", &application.destination);
    push_text(&mut status, "Policy", &application.sync_policy);
    status.push(Fact::text("Managed", application.resources_summary()));

    let repositories = application
        .sources
        .iter()
        .map(|source| source.repository.as_str())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let locations = application
        .sources
        .iter()
        .filter_map(
            |source| match (source.path.is_empty(), source.chart.is_empty()) {
                (false, _) => Some(source.path.as_str()),
                (true, false) => Some(source.chart.as_str()),
                (true, true) => None,
            },
        )
        .collect::<Vec<_>>()
        .join("\n");
    let revisions = application
        .sources
        .iter()
        .map(|source| source.target_revision.as_str())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let mut sources = Vec::new();
    push_id(&mut sources, "Repositories", &repositories);
    push_id(&mut sources, "Paths / charts", &locations);
    push_id(&mut sources, "Target revisions", &revisions);

    let mut sections = vec![Section {
        title: Some("GitOps".into()),
        facts: status,
    }];
    if !sources.is_empty() {
        sections.push(Section {
            title: Some("Sources".into()),
            facts: sources,
        });
    }
    sections
}

/// What an Argo CD ApplicationSet generates, read from the CR itself.
fn application_set(object: &Object) -> Vec<Section> {
    let application_set = crate::gitops::ApplicationSet::from_object(object);
    let mut facts = Vec::new();
    push_text(&mut facts, "Project", &application_set.project);
    push_text(
        &mut facts,
        "Generators",
        &application_set.generators.join(" · "),
    );
    facts.push(Fact::text(
        "Generated",
        format!("{} applications", application_set.applications),
    ));
    push_id(&mut facts, "Destination", &application_set.destination);
    if application_set.go_template {
        facts.push(Fact::text("Template", "Go template"));
    }
    push_text(&mut facts, "Strategy", &application_set.strategy);
    push_text(&mut facts, "Health", &application_set.health);
    vec![Section {
        title: Some("GitOps".into()),
        facts,
    }]
}

/// The boundaries an Argo CD AppProject puts around its Applications.
fn app_project(object: &Object) -> Vec<Section> {
    let project = crate::gitops::AppProject::from_object(object);
    let mut facts = Vec::new();
    push_text(&mut facts, "Description", &project.description);
    push_id(
        &mut facts,
        "Source repositories",
        &project.source_repositories.join("\n"),
    );
    push_id(
        &mut facts,
        "Source namespaces",
        &project.source_namespaces.join(" · "),
    );
    push_id(&mut facts, "Destinations", &project.destinations.join("\n"));
    push_id(
        &mut facts,
        "Cluster allow",
        &project.cluster_allow.join(" · "),
    );
    push_id(
        &mut facts,
        "Cluster deny",
        &project.cluster_deny.join(" · "),
    );
    push_id(
        &mut facts,
        "Namespace allow",
        &project.namespace_allow.join(" · "),
    );
    push_id(
        &mut facts,
        "Namespace deny",
        &project.namespace_deny.join(" · "),
    );
    push_id(&mut facts, "Roles", &project.roles.join(" · "));
    push_text(
        &mut facts,
        "Orphaned resources",
        &project.orphaned_resources,
    );
    if project.project_scoped_clusters_only {
        facts.push(Fact::text("Clusters", "Project-scoped only"));
    }
    if project.sync_windows > 0 {
        facts.push(Fact::text("Sync windows", project.sync_windows.to_string()));
    }
    vec![Section {
        title: Some("GitOps".into()),
        facts,
    }]
}

/// What an object is using right now, and — for a node — what share of it
/// that is.
fn usage(kind: &str, object: &Object, usage: &Metrics) -> Section {
    let mut facts = vec![
        Fact::id(
            "CPU",
            share(
                crate::time::cpu(usage.cpu_milli),
                allocatable(kind, object, "cpu").map(|total| usage.cpu_milli as f32 / total as f32),
            ),
        ),
        Fact::id(
            "Memory",
            share(
                crate::time::bytes(usage.memory_bytes),
                allocatable(kind, object, "memory")
                    .map(|total| usage.memory_bytes as f32 / total as f32),
            ),
        ),
    ];
    facts.retain(|fact| !fact.value.is_empty());
    Section {
        title: Some("Using".into()),
        facts,
    }
}

/// A node's allocatable amount of something, in the same unit the metrics
/// come in: milli-cores for CPU, bytes for memory.
fn allocatable(kind: &str, object: &Object, what: &str) -> Option<u64> {
    if kind != "Node" {
        return None;
    }
    let quantity_text = object.str_at(&format!("status.allocatable.{what}"));
    match what {
        "cpu" => quantity::cpu_milli(quantity_text),
        _ => quantity::bytes(quantity_text),
    }
    .filter(|total| *total > 0)
}

/// `143m` on its own, or `143m · 4% of allocatable` when there is something
/// to be a share of.
fn share(value: String, fraction: Option<f32>) -> String {
    match fraction {
        Some(fraction) => format!("{value}  ·  {}% of allocatable", (fraction * 100.0).round()),
        None => value,
    }
}

/// The block every object has.
fn metadata(object: &Object, now: DateTime<Utc>) -> Section {
    let mut facts = vec![Fact::id("Name", object.meta.name.clone())];
    if let Some(namespace) = &object.meta.namespace {
        facts.push(Fact::id("Namespace", namespace.clone()));
    }
    facts.push(Fact::text(
        "Created",
        crate::time::age(object.meta.created, now),
    ));
    if let Some(owner) = object.meta.controller() {
        // Up: from a pod to the thing that made it, which is the single most
        // common move in a cluster viewer.
        facts.push(
            Fact::id("Controlled by", format!("{}/{}", owner.kind, owner.name)).to(
                Target::Object {
                    key: owner.key(),
                    namespace: object.meta.namespace.clone(),
                    name: owner.name.clone(),
                },
            ),
        );
    }
    if !object.meta.labels.is_empty() {
        facts.push(Fact::id("Labels", pairs(&object.meta.labels)));
    }
    if !object.meta.uid.is_empty() {
        facts.push(Fact::id("UID", object.meta.uid.clone()));
    }
    Section { title: None, facts }
}

fn pod(object: &Object) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut facts = Vec::new();
    let node = object.str_at("spec.nodeName");
    if !node.is_empty() {
        facts.push(Fact::id("Node", node).to(Target::Object {
            key: ResourceKey::new("", "Node"),
            // Nodes are cluster-scoped: carrying the pod's namespace along
            // would ask for a node inside it, which does not exist.
            namespace: None,
            name: node.to_string(),
        }));
    }
    push_id(&mut facts, "Pod IP", object.str_at("status.podIP"));
    push_id(&mut facts, "Host IP", object.str_at("status.hostIP"));
    push_text(&mut facts, "QoS class", object.str_at("status.qosClass"));
    push_text(
        &mut facts,
        "Service account",
        object.str_at("spec.serviceAccountName"),
    );
    if !facts.is_empty() {
        sections.push(Section {
            title: Some("Placement".into()),
            facts,
        });
    }

    // One fact per container: what it runs, and what it is doing. The state
    // is read from `status.containerStatuses` by name rather than by index,
    // because the two lists are not guaranteed to be in the same order.
    let statuses = object.array_at("status.containerStatuses");
    let mut containers = Vec::new();
    for container in object.array_at("spec.containers") {
        let name = container
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let image = container
            .get("image")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status = statuses
            .iter()
            .find(|status| status.get("name").and_then(Value::as_str) == Some(name));
        let state = status.map(container_state).unwrap_or_default();
        let restarts = status
            .and_then(|status| status.get("restartCount"))
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let mut value = image.to_string();
        if !state.is_empty() {
            value.push_str(&format!("  ·  {state}"));
        }
        if restarts > 0 {
            value.push_str(&format!("  ·  {restarts} restarts"));
        }
        containers.push(Fact::id(name, value));
    }
    if !containers.is_empty() {
        sections.push(Section {
            title: Some("Containers".into()),
            facts: containers,
        });
    }
    sections
}

/// What one container is doing, in a word or two.
fn container_state(status: &Value) -> String {
    // `state` is a one-of: exactly one of `running`, `waiting`, `terminated`
    // is present, so the first entry is the answer.
    let Some((name, body)) = status
        .get("state")
        .and_then(Value::as_object)
        .and_then(|state| state.iter().next())
    else {
        return String::new();
    };
    body.get("reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
        .map(str::to_string)
        // `running` carries no reason, and "running" is the answer.
        .unwrap_or_else(|| name.clone())
}

fn node(object: &Object) -> Vec<Section> {
    let mut sections = Vec::new();
    let addresses: Vec<String> = object
        .array_at("status.addresses")
        .iter()
        .filter_map(|address| {
            let kind = address.get("type").and_then(Value::as_str)?;
            let value = address.get("address").and_then(Value::as_str)?;
            Some(format!("{kind} {value}"))
        })
        .collect();
    let mut facts = Vec::new();
    push_text(
        &mut facts,
        "Kubelet",
        object.str_at("status.nodeInfo.kubeletVersion"),
    );
    push_text(&mut facts, "OS", object.str_at("status.nodeInfo.osImage"));
    push_text(
        &mut facts,
        "Runtime",
        object.str_at("status.nodeInfo.containerRuntimeVersion"),
    );
    if !addresses.is_empty() {
        facts.push(Fact::id("Addresses", addresses.join("\n")));
    }
    if object.bool_at("spec.unschedulable") {
        facts.push(Fact::text("Scheduling", "Disabled (cordoned)"));
    }
    sections.push(Section {
        title: Some("Node".into()),
        facts,
    });

    let mut capacity = Vec::new();
    for (label, path) in [("CPU", "cpu"), ("Memory", "memory"), ("Pods", "pods")] {
        let allocatable = object.str_at(&format!("status.allocatable.{path}"));
        let total = object.str_at(&format!("status.capacity.{path}"));
        if allocatable.is_empty() && total.is_empty() {
            continue;
        }
        capacity.push(Fact::id(label, format!("{allocatable} of {total}")));
    }
    if !capacity.is_empty() {
        sections.push(Section {
            title: Some("Allocatable".into()),
            facts: capacity,
        });
    }
    sections
}

fn controller(object: &Object) -> Vec<Section> {
    let mut facts = Vec::new();
    push_text(&mut facts, "Strategy", object.str_at("spec.strategy.type"));
    push_text(
        &mut facts,
        "Update strategy",
        object.str_at("spec.updateStrategy.type"),
    );
    let selector = object
        .at("spec.selector.matchLabels")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| Some(format!("{key}={}", value.as_str()?)))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    // Down: the selector, as a way to the pods it selects. Answered the way a
    // person would answer it — by putting the selector in the filter box —
    // rather than by asking the apiserver a question it has no endpoint for.
    if !selector.is_empty() {
        let pods = Target::Filtered {
            key: ResourceKey::new("", "Pod"),
            namespace: object.meta.namespace.clone(),
            query: first_label(&selector),
        };
        facts.push(Fact::id("Selector", selector.clone()).to(pods.clone()));
        facts.push(Fact::id("Pod logs", "Select a pod").to(pods));
    }
    let images = object
        .array_at("spec.template.spec.containers")
        .iter()
        .filter_map(|container| container.get("image").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    push_id(&mut facts, "Images", &images);
    match facts.is_empty() {
        true => Vec::new(),
        false => vec![Section {
            title: Some("Template".into()),
            facts,
        }],
    }
}

fn service(object: &Object) -> Vec<Section> {
    let mut facts = Vec::new();
    push_text(&mut facts, "Type", object.str_at("spec.type"));
    push_id(&mut facts, "Cluster IP", object.str_at("spec.clusterIP"));
    let ports: Vec<String> = object
        .array_at("spec.ports")
        .iter()
        .map(|port| {
            let name = port.get("name").and_then(Value::as_str).unwrap_or("");
            let number = port.get("port").and_then(Value::as_i64).unwrap_or_default();
            let target = port
                .get("targetPort")
                .map(|target| match target {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            match name.is_empty() {
                true => format!("{number} → {target}"),
                false => format!("{name}  {number} → {target}"),
            }
        })
        .collect();
    if !ports.is_empty() {
        facts.push(Fact::id("Ports", ports.join("\n")));
    }
    let selector = object
        .at("spec.selector")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| Some(format!("{key}={}", value.as_str()?)))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !selector.is_empty() {
        facts.push(Fact::id("Selector", selector.clone()).to(Target::Filtered {
            key: ResourceKey::new("", "Pod"),
            namespace: object.meta.namespace.clone(),
            query: first_label(&selector),
        }));
    }
    vec![Section {
        title: Some("Service".into()),
        facts,
    }]
}

fn claim(object: &Object) -> Vec<Section> {
    let mut facts = Vec::new();
    push_text(&mut facts, "Phase", object.str_at("status.phase"));
    push_id(&mut facts, "Volume", object.str_at("spec.volumeName"));
    push_text(
        &mut facts,
        "Storage class",
        object.str_at("spec.storageClassName"),
    );
    push_id(
        &mut facts,
        "Requested",
        object.str_at("spec.resources.requests.storage"),
    );
    push_id(
        &mut facts,
        "Capacity",
        object.str_at("status.capacity.storage"),
    );
    let modes: Vec<&str> = object
        .array_at("spec.accessModes")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if !modes.is_empty() {
        facts.push(Fact::text("Access modes", modes.join(", ")));
    }
    vec![Section {
        title: Some("Claim".into()),
        facts,
    }]
}

/// A ConfigMap's or a Secret's keys.
///
/// Keys only, never values: a Secret's values are base64 of something the
/// reader did not ask this window to put on a screen behind them. Revealing
/// one is an action, and actions are M4.
fn keys(object: &Object) -> Vec<Section> {
    let mut keys: Vec<String> = object
        .at("data")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    match keys.is_empty() {
        true => Vec::new(),
        false => vec![Section {
            title: Some("Keys".into()),
            facts: keys
                .into_iter()
                .map(|key| Fact::id(key, String::new()))
                .collect(),
        }],
    }
}

/// The ports a pod's containers declare, as chips to forward to.
///
/// `containerPort` with its name when it has one, in the order the manifest
/// lists them and without duplicates — two containers may well both declare
/// 8080, and one chip is enough to forward it.
pub fn container_ports(object: &Object) -> Vec<(u16, String)> {
    let mut ports: Vec<(u16, String)> = Vec::new();
    for container in object.array_at("spec.containers") {
        for port in container
            .get("ports")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(number) = port
                .get("containerPort")
                .and_then(Value::as_u64)
                .and_then(|number| u16::try_from(number).ok())
            else {
                continue;
            };
            if ports.iter().any(|(known, _)| *known == number) {
                continue;
            }
            let label = port
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(|name| format!("{name} {number}"))
                .unwrap_or_else(|| number.to_string());
            ports.push((number, label));
        }
    }
    ports
}

/// The conditions, with the mark each one gets.
///
/// `Ready=False` is an error and `Ready=Unknown` is too, but the negative
/// conditions — `MemoryPressure`, `NetworkUnavailable` — mean the opposite:
/// `True` is the bad one. Reading a condition's polarity from its name is
/// what stops a healthy node from showing five red rows.
pub fn conditions(object: &Object) -> Vec<Condition> {
    object
        .array_at("status.conditions")
        .iter()
        .filter_map(|condition| {
            let kind = condition.get("type").and_then(Value::as_str)?.to_string();
            let status = condition
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_string();
            let reason = condition
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let good = match status.as_str() {
                "True" => !is_negative(&kind),
                "False" => is_negative(&kind),
                _ => {
                    return Some(Condition {
                        kind,
                        status,
                        reason,
                        level: Level::Unknown,
                    });
                }
            };
            let level = match good {
                true => Level::Ok,
                false => Level::Attention,
            };
            Some(Condition {
                kind,
                status,
                reason,
                level,
            })
        })
        .collect()
}

/// Whether a condition is one where `True` is the bad answer.
fn is_negative(kind: &str) -> bool {
    kind.ends_with("Pressure")
        || kind.ends_with("Unavailable")
        || kind.ends_with("Failed")
        || kind == "Failure"
}

/// The first `key=value` of a rendered selector.
///
/// The filter box is one fuzzy query, not a selector engine, so a selector of
/// several labels has to become one of them. The first is the one a person
/// would have typed: `app=api` narrows a namespace to a handful, and adding
/// `tier=backend` to it narrows nothing further in practice.
fn first_label(selector: &str) -> String {
    selector
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn push_text(facts: &mut Vec<Fact>, label: &str, value: &str) {
    if !value.is_empty() {
        facts.push(Fact::text(label, value));
    }
}

fn push_id(facts: &mut Vec<Fact>, label: &str, value: &str) {
    if !value.is_empty() {
        facts.push(Fact::id(label, value));
    }
}

/// A label or annotation map as `key=value` on one line each.
fn pairs(map: &std::collections::BTreeMap<String, String>) -> String {
    map.iter()
        .map(|(key, value)| match value.is_empty() {
            true => key.clone(),
            false => format!("{key}={value}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: serde_json::Value) -> Object {
        Object::new(value).unwrap()
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-07T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn facts(overview: &Overview) -> Vec<(&str, &str)> {
        overview
            .sections
            .iter()
            .flat_map(|section| section.facts.iter())
            .map(|fact| (fact.label.as_str(), fact.value.as_str()))
            .collect()
    }

    #[test]
    fn every_object_gets_the_metadata_block_whatever_kind_it_is() {
        let overview = overview(
            "Rollout",
            &object(
                json!({"metadata": {"name": "web", "namespace": "shop", "uid": "u",
                                        "creationTimestamp": "2026-09-07T09:00:00Z"}}),
            ),
            None,
            now(),
        );
        assert_eq!(overview.sections.len(), 1);
        assert!(overview.sections[0].title.is_none());
        let facts = facts(&overview);
        assert!(facts.contains(&("Name", "web")));
        assert!(facts.contains(&("Namespace", "shop")));
        assert!(facts.contains(&("Created", "3h")));
    }

    #[test]
    fn a_pod_says_where_it_is_and_what_each_container_is_doing() {
        let overview = overview(
            "Pod",
            &object(json!({
                "metadata": {"name": "api", "namespace": "shop"},
                "spec": {"nodeName": "node-1", "qosClass": "Burstable",
                         "containers": [
                             {"name": "api", "image": "ghcr.io/x/api:1.4"},
                             {"name": "proxy", "image": "envoy:1.31"}
                         ]},
                "status": {"podIP": "10.244.1.7", "qosClass": "Burstable",
                           "containerStatuses": [
                               {"name": "proxy", "restartCount": 0, "state": {"running": {}}},
                               {"name": "api", "restartCount": 4,
                                "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
                           ]}
            })),
            None,
            now(),
        );
        let facts = facts(&overview);
        assert!(facts.contains(&("Node", "node-1")));
        assert!(facts.contains(&("Pod IP", "10.244.1.7")));
        // Read by name, not by index: the two lists are in different orders.
        let api = facts.iter().find(|(label, _)| *label == "api").unwrap();
        assert!(api.1.contains("CrashLoopBackOff"), "{}", api.1);
        assert!(api.1.contains("4 restarts"), "{}", api.1);
        let proxy = facts.iter().find(|(label, _)| *label == "proxy").unwrap();
        assert!(proxy.1.contains("running"), "{}", proxy.1);
    }

    #[test]
    fn a_cordoned_node_says_so_in_words() {
        let overview = overview(
            "Node",
            &object(json!({
                "metadata": {"name": "node-2"},
                "spec": {"unschedulable": true},
                "status": {"nodeInfo": {"kubeletVersion": "v1.31.2"},
                           "capacity": {"cpu": "4", "memory": "8Gi"},
                           "allocatable": {"cpu": "3800m", "memory": "7.5Gi"}}
            })),
            None,
            now(),
        );
        let facts = facts(&overview);
        assert!(facts.iter().any(|(_, value)| value.contains("cordoned")));
        assert!(facts.contains(&("CPU", "3800m of 4")));
    }

    #[test]
    fn a_secret_shows_its_keys_and_never_its_values() {
        let overview = overview(
            "Secret",
            &object(json!({
                "metadata": {"name": "db"},
                "type": "Opaque",
                "data": {"password": "aHVudGVyMg==", "username": "cm9vdA=="}
            })),
            None,
            now(),
        );
        let facts = facts(&overview);
        assert!(facts.contains(&("password", "")));
        assert!(facts.contains(&("username", "")));
        assert!(
            !facts.iter().any(|(_, value)| value.contains("aHVudGVyMg")),
            "a secret's value must not reach the panel"
        );
    }

    fn linked<'a>(overview: &'a Overview, label: &str) -> &'a Fact {
        overview
            .sections
            .iter()
            .flat_map(|section| section.facts.iter())
            .find(|fact| fact.label == label)
            .unwrap_or_else(|| panic!("no fact called {label}"))
    }

    #[test]
    fn a_pod_points_up_at_what_made_it() {
        let overview = overview(
            "Pod",
            &object(json!({
                "metadata": {"name": "api-7d9f8c-2xk", "namespace": "shop",
                             "ownerReferences": [{"apiVersion": "apps/v1", "kind": "ReplicaSet",
                                                  "name": "api-7d9f8c", "uid": "rs",
                                                  "controller": true}]},
                "spec": {"nodeName": "node-1", "containers": [{"name": "api"}]}
            })),
            None,
            now(),
        );
        assert_eq!(
            linked(&overview, "Controlled by").link,
            Some(Target::Object {
                key: ResourceKey::new("apps", "ReplicaSet"),
                namespace: Some("shop".into()),
                name: "api-7d9f8c".into(),
            })
        );
    }

    #[test]
    fn a_pods_node_is_a_link_and_carries_no_namespace() {
        // Nodes are cluster-scoped: asking for one inside the pod's namespace
        // would be asking for something that does not exist.
        let overview = overview(
            "Pod",
            &object(json!({
                "metadata": {"name": "api", "namespace": "shop"},
                "spec": {"nodeName": "node-1", "containers": [{"name": "api"}]}
            })),
            None,
            now(),
        );
        assert_eq!(
            linked(&overview, "Node").link,
            Some(Target::Object {
                key: ResourceKey::new("", "Node"),
                namespace: None,
                name: "node-1".into(),
            })
        );
    }

    #[test]
    fn event_objects_do_not_offer_a_tab_for_events_about_the_event() {
        assert!(!has_related_events(&ResourceKey::new("", "Event")));
        assert!(!has_related_events(&ResourceKey::new(
            "events.k8s.io",
            "Event"
        )));
        assert!(has_related_events(&ResourceKey::new("", "Pod")));
        assert!(has_related_events(&ResourceKey::new("apps", "Deployment")));
    }

    #[test]
    fn a_controller_points_down_at_the_pods_it_selects() {
        let overview = overview(
            "Deployment",
            &object(json!({
                "metadata": {"name": "api", "namespace": "shop"},
                "spec": {"selector": {"matchLabels": {"app": "api", "tier": "backend"}},
                         "template": {"spec": {"containers": [{"name": "api", "image": "x:1"}]}}}
            })),
            None,
            now(),
        );
        assert_eq!(
            linked(&overview, "Selector").link,
            Some(Target::Filtered {
                key: ResourceKey::new("", "Pod"),
                namespace: Some("shop".into()),
                query: "app=api".into(),
            })
        );
        assert_eq!(
            linked(&overview, "Pod logs").link,
            Some(Target::Filtered {
                key: ResourceKey::new("", "Pod"),
                namespace: Some("shop".into()),
                query: "app=api".into(),
            })
        );
    }

    #[test]
    fn workload_logs_choose_matching_pods_newest_first() {
        let deployment = object(json!({
            "metadata": {"name": "api", "namespace": "shop"},
            "spec": {
                "selector": {"matchLabels": {"app": "api", "tier": "backend"}},
                "template": {"spec": {"containers": [{"name": "api"}]}}
            }
        }));
        let pods = vec![
            object(json!({
                "metadata": {"name": "wrong", "namespace": "shop",
                    "creationTimestamp": "2026-09-07T12:03:00Z",
                    "labels": {"app": "api", "tier": "frontend"}},
                "spec": {"containers": [{"name": "api"}]}
            })),
            object(json!({
                "metadata": {"name": "old", "namespace": "shop",
                    "creationTimestamp": "2026-09-07T12:01:00Z",
                    "labels": {"app": "api", "tier": "backend"}},
                "spec": {"containers": [{"name": "api"}]}
            })),
            object(json!({
                "metadata": {"name": "new", "namespace": "shop",
                    "creationTimestamp": "2026-09-07T12:02:00Z",
                    "labels": {"app": "api", "tier": "backend"}},
                "spec": {"containers": [{"name": "api"}]}
            })),
        ];

        assert_eq!(
            workload_log_pods(&deployment, &pods)
                .into_iter()
                .map(|pod| pod.meta.name.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "old"]
        );
    }

    #[test]
    fn workload_logs_honour_selector_expressions() {
        let workload = object(json!({
            "metadata": {"name": "api", "namespace": "shop"},
            "spec": {
                "selector": {"matchExpressions": [
                    {"key": "track", "operator": "In", "values": ["stable", "canary"]},
                    {"key": "debug", "operator": "DoesNotExist"}
                ]},
                "template": {"spec": {"containers": [{"name": "api"}]}}
            }
        }));
        let pods = vec![
            object(json!({"metadata": {"name": "stable", "namespace": "shop",
                "labels": {"track": "stable"}}})),
            object(json!({"metadata": {"name": "debug", "namespace": "shop",
                "labels": {"track": "canary", "debug": "true"}}})),
            object(json!({"metadata": {"name": "edge", "namespace": "shop",
                "labels": {"track": "edge"}}})),
        ];

        assert_eq!(
            workload_log_pods(&workload, &pods)
                .into_iter()
                .map(|pod| pod.meta.name.as_str())
                .collect::<Vec<_>>(),
            vec!["stable"]
        );
    }

    #[test]
    fn an_empty_selector_never_becomes_namespace_wide_logs() {
        let workload = object(json!({
            "metadata": {"name": "unsafe", "namespace": "shop"},
            "spec": {
                "selector": {},
                "template": {"spec": {"containers": [{"name": "app"}]}}
            }
        }));
        let pods = vec![object(json!({
            "metadata": {"name": "someone-elses", "namespace": "shop"}
        }))];

        assert!(!has_workload_logs(&workload));
        assert!(workload_log_pods(&workload, &pods).is_empty());
    }

    #[test]
    fn a_replication_controllers_legacy_selector_can_pick_log_pods() {
        let controller = object(json!({
            "metadata": {"name": "api", "namespace": "shop"},
            "spec": {
                "selector": {"app": "api"},
                "template": {"spec": {"containers": [{"name": "api"}]}}
            }
        }));
        let pods = vec![
            object(json!({"metadata": {"name": "api-1", "namespace": "shop",
                "labels": {"app": "api"}}})),
            object(json!({"metadata": {"name": "web-1", "namespace": "shop",
                "labels": {"app": "web"}}})),
        ];

        assert_eq!(
            workload_log_pods(&controller, &pods)
                .into_iter()
                .map(|pod| pod.meta.name.as_str())
                .collect::<Vec<_>>(),
            vec!["api-1"]
        );
    }

    #[test]
    fn workload_logs_support_exists_and_not_in_expressions() {
        let workload = object(json!({
            "metadata": {"name": "api", "namespace": "shop"},
            "spec": {
                "selector": {"matchExpressions": [
                    {"key": "app", "operator": "Exists"},
                    {"key": "track", "operator": "NotIn", "values": ["retired"]}
                ]},
                "template": {"spec": {"containers": [{"name": "api"}]}}
            }
        }));
        let pods = vec![
            object(json!({"metadata": {"name": "current", "namespace": "shop",
                "labels": {"app": "api", "track": "stable"}}})),
            object(json!({"metadata": {"name": "retired", "namespace": "shop",
                "labels": {"app": "api", "track": "retired"}}})),
            object(json!({"metadata": {"name": "unlabelled", "namespace": "shop"}})),
        ];

        assert_eq!(
            workload_log_pods(&workload, &pods)
                .into_iter()
                .map(|pod| pod.meta.name.as_str())
                .collect::<Vec<_>>(),
            vec!["current"]
        );
    }

    #[test]
    fn a_service_points_at_its_pods_the_same_way() {
        let overview = overview(
            "Service",
            &object(json!({
                "metadata": {"name": "api", "namespace": "shop"},
                "spec": {"type": "ClusterIP", "selector": {"app": "api"}}
            })),
            None,
            now(),
        );
        assert!(matches!(
            linked(&overview, "Selector").link,
            Some(Target::Filtered { .. })
        ));
    }

    #[test]
    fn an_argo_application_gets_a_native_gitops_summary() {
        let overview = overview_for(
            &ResourceKey::new("argoproj.io", "Application"),
            &object(json!({
                "metadata": {"name": "shop", "namespace": "argocd"},
                "spec": {
                    "project": "production",
                    "destination": {"server": "https://kubernetes.default.svc", "namespace": "shop"},
                    "source": {"repoURL": "https://github.com/acme/shop", "path": "deploy",
                               "targetRevision": "main"},
                    "syncPolicy": {"automated": {"prune": true}}
                },
                "status": {
                    "sync": {"status": "OutOfSync", "revision": "abc123"},
                    "health": {"status": "Degraded"},
                    "resources": [
                        {"group": "apps", "kind": "Deployment", "name": "api",
                         "status": "OutOfSync", "health": {"status": "Degraded"}}
                    ]
                }
            })),
            None,
            now(),
        );
        assert!(
            overview
                .sections
                .iter()
                .any(|section| section.title.as_deref() == Some("GitOps"))
        );
        let facts = facts(&overview);
        assert!(facts.contains(&("Project", "production")));
        assert!(facts.contains(&("Sync", "OutOfSync")));
        assert!(facts.contains(&("Managed", "1 resources · 1 out of sync · 1 unhealthy")));
        assert!(facts.contains(&("Repositories", "https://github.com/acme/shop")));
    }

    #[test]
    fn an_unrelated_application_kind_does_not_get_argo_fields() {
        let overview = overview_for(
            &ResourceKey::new("example.com", "Application"),
            &object(json!({
                "metadata": {"name": "something"},
                "spec": {"project": "not-argo"}
            })),
            None,
            now(),
        );
        assert!(
            !overview
                .sections
                .iter()
                .any(|section| section.title.as_deref() == Some("GitOps"))
        );
    }

    #[test]
    fn an_argo_application_set_gets_a_native_generation_summary() {
        let overview = overview_for(
            &ResourceKey::new("argoproj.io", "ApplicationSet"),
            &object(json!({
                "metadata": {"name": "environments", "namespace": "argocd"},
                "spec": {
                    "goTemplate": true,
                    "generators": [{"git": {}}, {"clusters": {}}],
                    "template": {"spec": {
                        "project": "platform",
                        "destination": {"name": "in-cluster", "namespace": "{{.namespace}}"}
                    }},
                    "strategy": {"type": "RollingSync"}
                },
                "status": {
                    "resourcesCount": 8,
                    "health": {"status": "Progressing"}
                }
            })),
            None,
            now(),
        );

        assert!(
            overview
                .sections
                .iter()
                .any(|section| section.title.as_deref() == Some("GitOps"))
        );
        let facts = facts(&overview);
        assert!(facts.contains(&("Project", "platform")));
        assert!(facts.contains(&("Generators", "git · clusters")));
        assert!(facts.contains(&("Generated", "8 applications")));
        assert!(facts.contains(&("Destination", "in-cluster · {{.namespace}}")));
        assert!(facts.contains(&("Template", "Go template")));
        assert!(facts.contains(&("Strategy", "RollingSync")));
    }

    #[test]
    fn an_argo_app_project_gets_a_native_policy_summary() {
        let overview = overview_for(
            &ResourceKey::new("argoproj.io", "AppProject"),
            &object(json!({
                "metadata": {"name": "production", "namespace": "argocd"},
                "spec": {
                    "description": "Production workloads",
                    "sourceRepos": ["https://github.com/acme/*"],
                    "destinations": [{"name": "production", "namespace": "shop-*"}],
                    "clusterResourceWhitelist": [{"group": "", "kind": "Namespace"}],
                    "namespaceResourceBlacklist": [{"group": "", "kind": "Secret"}],
                    "roles": [{"name": "read-only"}, {"name": "deploy"}],
                    "orphanedResources": {"warn": true, "ignore": []},
                    "permitOnlyProjectScopedClusters": true,
                    "syncWindows": [{"kind": "deny"}]
                }
            })),
            None,
            now(),
        );

        let facts = facts(&overview);
        assert!(facts.contains(&("Description", "Production workloads")));
        assert!(facts.contains(&("Source repositories", "https://github.com/acme/*")));
        assert!(facts.contains(&("Destinations", "production · shop-*")));
        assert!(facts.contains(&("Cluster allow", "core/Namespace")));
        assert!(facts.contains(&("Namespace deny", "core/Secret")));
        assert!(facts.contains(&("Roles", "read-only · deploy")));
        assert!(facts.contains(&("Orphaned resources", "Warnings enabled · 0 ignored")));
        assert!(facts.contains(&("Clusters", "Project-scoped only")));
        assert!(facts.contains(&("Sync windows", "1")));
    }

    #[test]
    fn a_selector_of_several_labels_becomes_the_first_one() {
        // The filter box is one fuzzy query, not a selector engine.
        assert_eq!(first_label("app=api, tier=backend"), "app=api");
        assert_eq!(first_label("app=api"), "app=api");
        assert_eq!(first_label(""), "");
    }

    #[test]
    fn a_cluster_with_no_metrics_gets_no_usage_section_rather_than_a_row_of_zeroes() {
        // A pod using no CPU and a cluster with no metrics server look
        // identical as zeroes and mean opposite things.
        let overview = overview(
            "Pod",
            &object(json!({"metadata": {"name": "p"}})),
            None,
            now(),
        );
        assert!(
            !overview
                .sections
                .iter()
                .any(|section| section.title.as_deref() == Some("Using"))
        );
    }

    #[test]
    fn a_pod_says_what_it_is_using() {
        let usage = Metrics {
            name: "api".into(),
            namespace: Some("shop".into()),
            cpu_milli: 143,
            memory_bytes: 268_435_456,
        };
        let overview = overview(
            "Pod",
            &object(json!({"metadata": {"name": "api", "namespace": "shop"}})),
            Some(&usage),
            now(),
        );
        assert_eq!(linked(&overview, "CPU").value, "143m");
        assert_eq!(linked(&overview, "Memory").value, "256Mi");
    }

    #[test]
    fn a_node_says_what_share_of_itself_that_is() {
        let usage = Metrics {
            name: "node-1".into(),
            namespace: None,
            cpu_milli: 1900,
            memory_bytes: 4 * 1024 * 1024 * 1024,
        };
        let overview = overview(
            "Node",
            &object(json!({
                "metadata": {"name": "node-1"},
                "status": {"allocatable": {"cpu": "3800m", "memory": "8Gi"},
                           "capacity": {"cpu": "4", "memory": "8Gi"}}
            })),
            Some(&usage),
            now(),
        );
        assert!(
            linked(&overview, "CPU").value.contains("50%"),
            "{}",
            linked(&overview, "CPU").value
        );
        assert!(linked(&overview, "Memory").value.contains("50%"));
    }

    #[test]
    fn a_node_with_nothing_allocatable_reports_the_number_without_a_share() {
        let usage = Metrics {
            name: "node-1".into(),
            namespace: None,
            cpu_milli: 100,
            memory_bytes: 1024,
        };
        let overview = overview(
            "Node",
            &object(json!({"metadata": {"name": "node-1"}})),
            Some(&usage),
            now(),
        );
        assert_eq!(linked(&overview, "CPU").value, "100m");
    }

    #[test]
    fn a_pods_container_ports_become_chips_named_when_the_manifest_names_them() {
        let pod = object(json!({
            "metadata": {"name": "api", "namespace": "shop"},
            "spec": {"containers": [
                {"name": "api", "ports": [{"containerPort": 8080, "name": "http"},
                                          {"containerPort": 9090}]},
                {"name": "proxy", "ports": [{"containerPort": 8080, "name": "http"}]}
            ]}
        }));
        assert_eq!(
            container_ports(&pod),
            vec![(8080, "http 8080".to_string()), (9090, "9090".to_string())]
        );
        let none =
            object(json!({"metadata": {"name": "x"}, "spec": {"containers": [{"name": "a"}]}}));
        assert!(container_ports(&none).is_empty());
    }

    #[test]
    fn a_negative_condition_is_good_when_it_is_false() {
        let node = object(json!({"metadata": {"name": "n"}, "status": {"conditions": [
            {"type": "Ready", "status": "True"},
            {"type": "MemoryPressure", "status": "False"},
            {"type": "DiskPressure", "status": "True"},
            {"type": "NetworkUnavailable", "status": "Unknown"}
        ]}}));
        let conditions = conditions(&node);
        assert_eq!(conditions[0].level, Level::Ok);
        // False pressure is the healthy answer.
        assert_eq!(conditions[1].level, Level::Ok);
        assert_eq!(conditions[2].level, Level::Attention);
        assert_eq!(conditions[3].level, Level::Unknown);
    }

    #[test]
    fn a_condition_carries_the_controllers_reason_when_there_is_one() {
        let deployment = object(json!({"metadata": {"name": "d"}, "status": {"conditions": [
            {"type": "Available", "status": "False", "reason": "MinimumReplicasUnavailable"}
        ]}}));
        let conditions = conditions(&deployment);
        assert_eq!(conditions[0].reason, "MinimumReplicasUnavailable");
        assert_eq!(conditions[0].level, Level::Attention);
    }

    #[test]
    fn an_object_with_nothing_to_add_gets_no_empty_sections() {
        let overview = overview(
            "ConfigMap",
            &object(json!({"metadata": {"name": "c"}})),
            None,
            now(),
        );
        assert_eq!(overview.sections.len(), 1);
        assert!(overview.conditions.is_empty());
    }
}
