//! The centre column's columns.
//!
//! One table draws every kind (`AGENTS.md` rule 8), so the per-kind knowledge
//! is *data*: a list of columns, each paired with the thing to read out of
//! the object. Titles and cells come from the same list, which is what makes
//! it impossible for them to drift apart — the failure mode of every table
//! that keeps its headers in one place and its rows in another.
//!
//! The column sets are `kubectl get`'s, in `kubectl`'s order, because the
//! value of a column called `READY` showing `2/2` is that the reader has
//! already learnt it somewhere else. A kind with no set of its own gets Name,
//! Namespace and Age — which is what `kubectl` prints for a custom resource
//! too.

use chrono::{DateTime, Utc};
use kirikumo_kube::{
    ApiResource, Health, Level, Object, PrinterColumn, ResourceKey, health, jsonpath, quantity,
};
use serde_json::Value;
use std::collections::HashMap;

/// How a column takes its share of the table's width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    /// A share of what is left after the fixed columns.
    Flex(f32),
    /// Exactly this many pixels: the narrow columns whose contents are one
    /// or two characters and which must not stretch.
    Fixed(f32),
}

/// One column.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// The heading, in `kubectl`'s spelling — or a CRD's own.
    pub title: String,
    /// How wide.
    pub width: Width,
    /// Whether the cells are identifiers, which are drawn in the mono family
    /// so a column of them lines up (`docs/ui.md` §2).
    pub mono: bool,
    /// Whether the cells are numbers, which are drawn right-aligned.
    pub numeric: bool,
}

impl Column {
    fn text(title: &str, flex: f32) -> Self {
        Self {
            title: title.to_string(),
            width: Width::Flex(flex),
            mono: false,
            numeric: false,
        }
    }

    fn id(title: &str, flex: f32) -> Self {
        Self {
            title: title.to_string(),
            width: Width::Flex(flex),
            mono: true,
            numeric: false,
        }
    }

    fn num(title: &str, pixels: f32) -> Self {
        Self {
            title: title.to_string(),
            width: Width::Fixed(pixels),
            mono: true,
            numeric: true,
        }
    }

    fn fixed(title: &str, pixels: f32) -> Self {
        Self {
            title: title.to_string(),
            width: Width::Fixed(pixels),
            mono: true,
            numeric: false,
        }
    }
}

/// What to read out of an object for one column.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    /// A CRD's own column: a JSONPath into the object.
    JsonPath(String),
    /// A CRD's own column of type `date`: a timestamp at a JSONPath, shown
    /// as an age.
    JsonPathDate(String),
    /// `metadata.name`.
    Name,
    /// `metadata.namespace`.
    Namespace,
    /// How long ago it was created, `kubectl`'s way.
    Age,
    /// The word beside the health mark.
    Status,
    /// A pod's ready containers over its total.
    Ready,
    /// A pod's restarts, summed.
    Restarts,
    /// A string at a dotted path.
    Str(&'static str),
    /// A number at a dotted path, printed as `0` when absent — which is what
    /// the apiserver means by omitting it.
    Int(&'static str),
    /// How many entries an object or array at a path has.
    Count(&'static str),
    /// A node's roles, from its `node-role.kubernetes.io/*` labels.
    NodeRoles,
    /// A service's ports, as `80:30001/TCP`.
    ServicePorts,
    /// A service's external address, or `<none>`/`<pending>`.
    ServiceExternalIp,
    /// An ingress's hosts, comma-separated.
    IngressHosts,
    /// The images a pod or a controller's template runs.
    Images,
    /// A job's completions, as `1/1`.
    JobCompletions,
    /// A cron job's `spec.suspend`, as `True`/`False`, which is how
    /// `kubectl` prints it.
    Suspend,
    /// A timestamp at a path, as an age.
    Since(&'static str),
    /// A quantity at a path, printed in binary units.
    Bytes(&'static str),
    /// An autoscaler's target, as `Deployment/api`.
    ScaleTarget,
    /// A binding's role, as `ClusterRole/view`.
    RoleRef,
    /// An endpoints object's addresses.
    Endpoints,
}

/// The columns for one kind, and what fills them.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSet {
    /// The API group, empty for core resources or a kind-only set.
    group: String,
    kind: String,
    columns: Vec<(Column, Cell)>,
    /// Which column the table sorts by when nothing has been clicked.
    default_sort: usize,
    /// Whether that default is ascending.
    default_ascending: bool,
}

/// What `kubectl` prints where a field is empty.
const NONE: &str = "<none>";

impl ColumnSet {
    /// The columns for a kind.
    ///
    /// `show_namespace` is true when the table is listing every namespace at
    /// once, which is the only time a NAMESPACE column earns its width.
    pub fn for_kind(kind: &str, namespaced: bool, show_namespace: bool) -> Self {
        let mut columns: Vec<(Column, Cell)> = vec![(Column::id("NAME", 3.0), Cell::Name)];
        if namespaced && show_namespace {
            columns.push((Column::id("NAMESPACE", 1.4), Cell::Namespace));
        }
        // Age descending for the kinds that churn — the newest pod is the one
        // that just crashed — and name ascending for the ones that do not.
        let mut sort_by_age = false;
        match kind {
            "Pod" => {
                columns.extend([
                    (Column::num("READY", 62.), Cell::Ready),
                    (Column::text("STATUS", 1.3), Cell::Status),
                    (Column::num("RESTARTS", 76.), Cell::Restarts),
                    (Column::id("IP", 1.0), Cell::Str("status.podIP")),
                    (Column::id("NODE", 1.2), Cell::Str("spec.nodeName")),
                ]);
                sort_by_age = true;
            }
            "Deployment" | "StatefulSet" => columns.extend([
                (Column::num("READY", 62.), Cell::Status),
                (
                    Column::num("UP-TO-DATE", 92.),
                    Cell::Int("status.updatedReplicas"),
                ),
                (
                    Column::num("AVAILABLE", 82.),
                    Cell::Int("status.availableReplicas"),
                ),
                (Column::id("IMAGES", 2.0), Cell::Images),
            ]),
            "ReplicaSet" => columns.extend([
                (Column::num("DESIRED", 72.), Cell::Int("spec.replicas")),
                (Column::num("CURRENT", 72.), Cell::Int("status.replicas")),
                (Column::num("READY", 62.), Cell::Int("status.readyReplicas")),
            ]),
            "DaemonSet" => columns.extend([
                (
                    Column::num("DESIRED", 72.),
                    Cell::Int("status.desiredNumberScheduled"),
                ),
                (Column::num("READY", 62.), Cell::Int("status.numberReady")),
                (
                    Column::num("UP-TO-DATE", 92.),
                    Cell::Int("status.updatedNumberScheduled"),
                ),
                (
                    Column::num("AVAILABLE", 82.),
                    Cell::Int("status.numberAvailable"),
                ),
            ]),
            "Job" => {
                columns.extend([
                    (Column::num("COMPLETIONS", 100.), Cell::JobCompletions),
                    (Column::text("STATUS", 1.0), Cell::Status),
                ]);
                sort_by_age = true;
            }
            "CronJob" => columns.extend([
                (Column::id("SCHEDULE", 1.2), Cell::Str("spec.schedule")),
                (Column::fixed("SUSPEND", 74.), Cell::Suspend),
                (Column::num("ACTIVE", 62.), Cell::Count("status.active")),
                (
                    Column::fixed("LAST SCHEDULE", 104.),
                    Cell::Since("status.lastScheduleTime"),
                ),
            ]),
            "Node" => columns.extend([
                (Column::text("STATUS", 1.2), Cell::Status),
                (Column::text("ROLES", 1.0), Cell::NodeRoles),
                (
                    Column::id("VERSION", 1.0),
                    Cell::Str("status.nodeInfo.kubeletVersion"),
                ),
            ]),
            "Namespace" => columns.push((Column::text("STATUS", 1.0), Cell::Status)),
            "Service" => columns.extend([
                (Column::text("TYPE", 1.0), Cell::Str("spec.type")),
                (Column::id("CLUSTER-IP", 1.1), Cell::Str("spec.clusterIP")),
                (Column::id("EXTERNAL-IP", 1.1), Cell::ServiceExternalIp),
                (Column::id("PORTS", 1.3), Cell::ServicePorts),
            ]),
            "Endpoints" => columns.push((Column::id("ENDPOINTS", 3.0), Cell::Endpoints)),
            "Ingress" => columns.extend([
                (
                    Column::text("CLASS", 0.9),
                    Cell::Str("spec.ingressClassName"),
                ),
                (Column::id("HOSTS", 2.0), Cell::IngressHosts),
            ]),
            "ConfigMap" => columns.push((Column::num("DATA", 62.), Cell::Count("data"))),
            "Secret" => columns.extend([
                (Column::text("TYPE", 1.4), Cell::Str("type")),
                (Column::num("DATA", 62.), Cell::Count("data")),
            ]),
            "PersistentVolumeClaim" => columns.extend([
                (Column::text("STATUS", 0.9), Cell::Status),
                (Column::id("VOLUME", 1.6), Cell::Str("spec.volumeName")),
                (
                    Column::num("CAPACITY", 82.),
                    Cell::Bytes("status.capacity.storage"),
                ),
                (
                    Column::text("STORAGECLASS", 1.1),
                    Cell::Str("spec.storageClassName"),
                ),
            ]),
            "PersistentVolume" => columns.extend([
                (
                    Column::num("CAPACITY", 82.),
                    Cell::Bytes("spec.capacity.storage"),
                ),
                (Column::text("STATUS", 0.9), Cell::Status),
                (Column::id("CLAIM", 1.6), Cell::Str("spec.claimRef.name")),
                (
                    Column::text("STORAGECLASS", 1.1),
                    Cell::Str("spec.storageClassName"),
                ),
            ]),
            "StorageClass" => {
                columns.push((Column::id("PROVISIONER", 2.0), Cell::Str("provisioner")))
            }
            "ServiceAccount" => columns.push((Column::num("SECRETS", 72.), Cell::Count("secrets"))),
            "HorizontalPodAutoscaler" => columns.extend([
                (Column::id("REFERENCE", 1.6), Cell::ScaleTarget),
                (Column::num("MINPODS", 76.), Cell::Int("spec.minReplicas")),
                (Column::num("MAXPODS", 76.), Cell::Int("spec.maxReplicas")),
                (
                    Column::num("REPLICAS", 82.),
                    Cell::Int("status.currentReplicas"),
                ),
            ]),
            "RoleBinding" | "ClusterRoleBinding" => {
                columns.push((Column::id("ROLE", 1.8), Cell::RoleRef))
            }
            "Event" => {
                columns.extend([
                    (Column::text("TYPE", 0.7), Cell::Str("type")),
                    (Column::id("REASON", 1.0), Cell::Str("reason")),
                    (Column::id("OBJECT", 1.4), Cell::Str("involvedObject.name")),
                    (Column::text("MESSAGE", 3.0), Cell::Str("message")),
                ]);
                sort_by_age = true;
            }
            _ => {}
        }
        columns.push((Column::fixed("AGE", 74.), Cell::Age));
        let age = columns.len() - 1;
        Self {
            group: String::new(),
            kind: kind.to_string(),
            columns,
            default_sort: if sort_by_age { age } else { 0 },
            // Ascending either way, and it means the useful thing in both:
            // A→Z by name, and *youngest first* by age, because a smaller
            // age is a newer object and the newest pod is the one that just
            // crashed.
            default_ascending: true,
        }
    }

    /// The columns for a resource, given whether the table is showing every
    /// namespace, and the columns its CRD declares if it is a custom kind.
    ///
    /// A kind with a set of its own here keeps it. Everything else — every
    /// custom resource — gets the columns its own authors designed, between
    /// NAME and AGE, exactly as `kubectl get -o wide` prints them; a CRD that
    /// declares none gets the plain table.
    pub fn for_resource(
        resource: &ApiResource,
        show_namespace: bool,
        printer: Option<&[PrinterColumn]>,
    ) -> Self {
        let mut set = Self::for_kind(&resource.kind, resource.namespaced, show_namespace);
        set.group.clone_from(&resource.group);
        let Some(printer) = printer.filter(|columns| !columns.is_empty()) else {
            return set;
        };
        if set.is_generic() {
            let age = set.columns.pop();
            for column in printer {
                let cell = match column.is_date() {
                    true => Cell::JsonPathDate(column.json_path.clone()),
                    false => Cell::JsonPath(column.json_path.clone()),
                };
                let heading = column.name.to_uppercase();
                let drawn = match (column.is_numeric(), column.is_date()) {
                    (true, _) => Column::num(&heading, 82.),
                    (_, true) => Column::fixed(&heading, 74.),
                    _ => Column::text(&heading, 1.0),
                };
                set.columns.push((drawn, cell));
            }
            set.columns.extend(age);
        }
        set
    }

    /// Whether this is the fallback table — NAME, maybe NAMESPACE, AGE — with
    /// nothing kind-specific in it, which is when a CRD's own columns are
    /// worth more than ours.
    pub fn is_generic(&self) -> bool {
        self.columns
            .iter()
            .all(|(_, cell)| matches!(cell, Cell::Name | Cell::Namespace | Cell::Age))
    }

    /// The kind these columns are for.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The headings, in order.
    pub fn columns(&self) -> impl Iterator<Item = &Column> {
        self.columns.iter().map(|(column, _)| column)
    }

    /// How many columns there are.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Whether there are no columns, which cannot happen — NAME and AGE are
    /// always there — but which clippy asks about.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// The column and direction the table sorts by until the reader says
    /// otherwise.
    pub fn default_sort(&self) -> (usize, bool) {
        (self.default_sort, self.default_ascending)
    }

    /// Whether a column holds an age, which sorts by the underlying
    /// timestamp rather than by the string: `9m59s` is younger than `10m`,
    /// and no string comparison will ever say so.
    pub fn is_age(&self, column: usize) -> bool {
        matches!(self.columns.get(column), Some((_, Cell::Age)))
    }

    /// One object's cells, in column order.
    pub fn cells(&self, object: &Object, now: DateTime<Utc>) -> Vec<String> {
        self.columns
            .iter()
            .map(|(_, cell)| render(cell, &self.kind, object, now))
            .collect()
    }

    /// One row, ready to draw.
    pub fn row(&self, object: &Object, now: DateTime<Utc>) -> Row {
        let cells = self.cells(object, now);
        // What the filter box matches against: every cell, plus the labels,
        // because "app=api" is a thing people type.
        let mut haystack = cells.join(" ");
        for (name, value) in &object.meta.labels {
            haystack.push(' ');
            haystack.push_str(name);
            haystack.push('=');
            haystack.push_str(value);
        }
        Row {
            key: row_key(object),
            name: object.meta.name.clone(),
            namespace: object.meta.namespace.clone(),
            health: health::of_resource(&ResourceKey::new(&self.group, &self.kind), object),
            created: object.meta.created,
            cells,
            haystack: haystack.to_lowercase(),
            version: object.meta.resource_version.clone(),
        }
    }

    /// Rows for a whole list, reusing the ones that have not changed.
    ///
    /// A watch delivers one changed object at a time, and rebuilding every
    /// row for each event is how a live table on a busy namespace becomes a
    /// space heater: four thousand objects times seven formatted cells, tens
    /// of times a second. An object whose `resourceVersion` has not moved
    /// cannot have changed, so its row is carried over untouched and the only
    /// work is a hash lookup.
    ///
    /// `previous` may be in any order and may hold rows for objects that have
    /// since gone; both are ignored. The answer is always in `objects`' order
    /// and holds exactly one row per object, so a caller sorts afterwards.
    pub fn rows(&self, objects: &[Object], previous: &[Row], now: DateTime<Utc>) -> Vec<Row> {
        let kept: HashMap<&str, &Row> =
            previous.iter().map(|row| (row.key.as_str(), row)).collect();
        objects
            .iter()
            .map(|object| {
                let identity = object.meta.identity();
                match kept.get(identity.as_str()) {
                    // The AGE cell is the one thing that goes stale without
                    // the object changing, so a reused row is only right
                    // between watch events — which is exactly when it is
                    // used. A refresh rebuilds from scratch.
                    Some(row) if row.version == object.meta.resource_version => (*row).clone(),
                    _ => self.row(object, now),
                }
            })
            .collect()
    }
}

/// The identity of a row.
///
/// The same identity the watch matches its events by
/// ([`kirikumo_kube::ObjectMeta::identity`]) — deliberately one function, not
/// two, because two spellings of "the same object" is how a live table grows
/// duplicates.
pub fn row_key(object: &Object) -> String {
    object.meta.identity()
}

/// One object as the table draws it.
///
/// Everything is a string by the time it gets here: a cell is formatted once,
/// when the object lands, and never during a scroll (`AGENTS.md` rule 7).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The object's identity.
    pub key: String,
    /// Its name, for the detail panel.
    pub name: String,
    /// Its namespace, for the detail panel.
    pub namespace: Option<String>,
    /// The mark and the word.
    pub health: Health,
    /// The cells, one per column.
    pub cells: Vec<String>,
    /// When it was created, which is what an AGE column sorts by.
    pub created: Option<DateTime<Utc>>,
    /// Every cell and label, lowercased, for the filter box.
    pub haystack: String,
    /// The `resourceVersion` this row was built from.
    ///
    /// What makes a live table cheap: a watch event changes one object, and
    /// every row whose version has not moved is reused rather than
    /// reformatted. See [`ColumnSet::rows`].
    pub version: String,
}

impl Row {
    /// Whether this row is one the reader should look at.
    pub fn is_bad(&self) -> bool {
        self.health.level.is_bad()
    }
}

/// Sort rows by a column.
///
/// Ages sort by their timestamp, numbers by their value, and everything else
/// by its text, case-insensitively. A cell that is `<none>` sorts last
/// whichever way the column is pointing, because an absence is not a value.
pub fn sort(rows: &mut [Row], columns: &ColumnSet, column: usize, ascending: bool) {
    if columns.is_age(column) {
        // A newer object has a larger timestamp and a *smaller* age, so
        // "ascending age" is descending time.
        rows.sort_by(|a, b| match ascending {
            true => b.created.cmp(&a.created),
            false => a.created.cmp(&b.created),
        });
        return;
    }
    rows.sort_by(|a, b| {
        let left = a.cells.get(column).map(String::as_str).unwrap_or_default();
        let right = b.cells.get(column).map(String::as_str).unwrap_or_default();
        let order = match (numeric(left), numeric(right)) {
            (Some(left), Some(right)) => left.total_cmp(&right),
            _ => left.to_lowercase().cmp(&right.to_lowercase()),
        };
        match ascending {
            true => order,
            false => order.reverse(),
        }
    });
}

/// A cell that is only a number, for sorting.
fn numeric(cell: &str) -> Option<f64> {
    cell.parse().ok()
}

/// Read one cell out of an object.
fn render(cell: &Cell, kind: &str, object: &Object, now: DateTime<Utc>) -> String {
    match cell {
        Cell::JsonPath(path) => jsonpath::cell(&object.raw, path),
        Cell::JsonPathDate(path) => {
            let text = jsonpath::cell(&object.raw, path);
            match DateTime::parse_from_rfc3339(&text) {
                Ok(time) => crate::time::age(Some(time.with_timezone(&Utc)), now),
                Err(_) if text.is_empty() => NONE.to_string(),
                Err(_) => text,
            }
        }
        Cell::Name => object.meta.name.clone(),
        Cell::Namespace => object.meta.namespace.clone().unwrap_or_default(),
        Cell::Age => crate::time::age(object.meta.created, now),
        Cell::Status => health::of(kind, object).word,
        Cell::Ready => {
            let (ready, total) = health::ready_containers(object);
            format!("{ready}/{total}")
        }
        Cell::Restarts => health::restarts(object).to_string(),
        Cell::Str(path) => or_none(object.str_at(path)),
        Cell::Int(path) => object.int_at(path).to_string(),
        Cell::Count(path) => match object.at(path) {
            Some(Value::Object(map)) => map.len().to_string(),
            Some(Value::Array(items)) => items.len().to_string(),
            _ => "0".to_string(),
        },
        Cell::NodeRoles => node_roles(object),
        Cell::ServicePorts => service_ports(object),
        Cell::ServiceExternalIp => service_external_ip(object),
        Cell::IngressHosts => ingress_hosts(object),
        Cell::Images => images(object),
        Cell::JobCompletions => {
            let completions = object
                .at("spec.completions")
                .and_then(Value::as_i64)
                .unwrap_or(1);
            format!("{}/{completions}", object.int_at("status.succeeded"))
        }
        // `kubectl` prints a boolean as `True`/`False` here, not `true`.
        Cell::Suspend => match object.bool_at("spec.suspend") {
            true => "True".to_string(),
            false => "False".to_string(),
        },
        Cell::Since(path) => match object
            .at(path)
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        {
            Some(time) => crate::time::age(Some(time.with_timezone(&Utc)), now),
            None => NONE.to_string(),
        },
        Cell::Bytes(path) => match object.str_at(path) {
            // The quantity is shown as written — `50Gi`, not `53687091200` —
            // because that is the number in the manifest.
            "" => NONE.to_string(),
            quantity => quantity.to_string(),
        },
        Cell::ScaleTarget => {
            let kind = object.str_at("spec.scaleTargetRef.kind");
            let name = object.str_at("spec.scaleTargetRef.name");
            match kind.is_empty() {
                true => NONE.to_string(),
                false => format!("{kind}/{name}"),
            }
        }
        Cell::RoleRef => {
            let kind = object.str_at("roleRef.kind");
            let name = object.str_at("roleRef.name");
            match kind.is_empty() {
                true => NONE.to_string(),
                false => format!("{kind}/{name}"),
            }
        }
        Cell::Endpoints => endpoints(object),
    }
}

/// An empty string as `kubectl` prints one.
fn or_none(value: &str) -> String {
    match value.is_empty() {
        true => NONE.to_string(),
        false => value.to_string(),
    }
}

/// A node's roles, from the labels the control plane puts on it.
fn node_roles(object: &Object) -> String {
    const PREFIX: &str = "node-role.kubernetes.io/";
    let mut roles: Vec<&str> = object
        .meta
        .labels
        .keys()
        .filter_map(|label| label.strip_prefix(PREFIX))
        .filter(|role| !role.is_empty())
        .collect();
    roles.sort_unstable();
    match roles.is_empty() {
        true => NONE.to_string(),
        false => roles.join(","),
    }
}

/// A service's ports, as `kubectl` writes them: `80:30001/TCP`.
fn service_ports(object: &Object) -> String {
    let ports: Vec<String> = object
        .array_at("spec.ports")
        .iter()
        .map(|port| {
            let number = port.get("port").and_then(Value::as_i64).unwrap_or_default();
            let protocol = port
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or("TCP");
            match port.get("nodePort").and_then(Value::as_i64) {
                Some(node_port) => format!("{number}:{node_port}/{protocol}"),
                None => format!("{number}/{protocol}"),
            }
        })
        .collect();
    match ports.is_empty() {
        true => NONE.to_string(),
        false => ports.join(","),
    }
}

/// A service's external address.
///
/// `<pending>` for a load balancer that has not been given one, which is the
/// single most-asked question about a `Service` and deserves its own word.
fn service_external_ip(object: &Object) -> String {
    let ingress: Vec<String> = object
        .array_at("status.loadBalancer.ingress")
        .iter()
        .filter_map(|entry| {
            entry
                .get("ip")
                .or_else(|| entry.get("hostname"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    if !ingress.is_empty() {
        return ingress.join(",");
    }
    let external: Vec<String> = object
        .array_at("spec.externalIPs")
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    if !external.is_empty() {
        return external.join(",");
    }
    match object.str_at("spec.type") {
        "LoadBalancer" => "<pending>".to_string(),
        _ => NONE.to_string(),
    }
}

/// An ingress's hosts.
fn ingress_hosts(object: &Object) -> String {
    let hosts: Vec<String> = object
        .array_at("spec.rules")
        .iter()
        .filter_map(|rule| rule.get("host").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    match hosts.is_empty() {
        true => "*".to_string(),
        false => hosts.join(","),
    }
}

/// The images an object runs, from a pod spec or from a controller's
/// template.
fn images(object: &Object) -> String {
    let containers = match object.at("spec.template.spec.containers") {
        Some(_) => object.array_at("spec.template.spec.containers"),
        None => object.array_at("spec.containers"),
    };
    let images: Vec<String> = containers
        .iter()
        .filter_map(|container| container.get("image").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    match images.is_empty() {
        true => NONE.to_string(),
        false => images.join(","),
    }
}

/// An endpoints object's addresses, as `10.0.0.1:8080,10.0.0.2:8080`.
fn endpoints(object: &Object) -> String {
    let mut listed = Vec::new();
    for subset in object.array_at("subsets") {
        let ports: Vec<String> = subset
            .get("ports")
            .and_then(Value::as_array)
            .map(|ports| {
                ports
                    .iter()
                    .filter_map(|port| port.get("port").and_then(Value::as_i64))
                    .map(|port| port.to_string())
                    .collect()
            })
            .unwrap_or_default();
        for address in subset
            .get("addresses")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(ip) = address.get("ip").and_then(Value::as_str) else {
                continue;
            };
            match ports.first() {
                Some(port) => listed.push(format!("{ip}:{port}")),
                None => listed.push(ip.to_string()),
            }
        }
    }
    match listed.is_empty() {
        true => NONE.to_string(),
        false => listed.join(","),
    }
}

/// A quantity as a byte count, for the places a number reads better than the
/// manifest's own spelling — a node's memory beside what it is using.
pub fn as_bytes(quantity_text: &str) -> Option<String> {
    quantity::bytes(quantity_text).map(crate::time::bytes)
}

/// The share of a capacity something is using, as a percentage, when both are
/// known. `None` rather than zero when either is missing: a bar drawn at zero
/// because a number was absent is a lie.
pub fn percent(used: u64, capacity: u64) -> Option<f32> {
    (capacity > 0).then(|| (used as f32 / capacity as f32 * 100.0).min(999.0))
}

/// The health of a whole table, for the sidebar's count: how many rows are
/// worth looking at.
pub fn bad_rows(rows: &[Row]) -> usize {
    rows.iter().filter(|row| row.is_bad()).count()
}

/// The worst level in a table, for a summary mark.
pub fn worst(rows: &[Row]) -> Level {
    rows.iter()
        .map(|row| row.health.level)
        .max_by_key(|level| match level {
            Level::Error => 4,
            Level::Attention => 3,
            Level::Working => 2,
            Level::Ok => 1,
            Level::Unknown => 0,
        })
        .unwrap_or(Level::Unknown)
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

    fn pod() -> Object {
        object(json!({
            "metadata": {"name": "api-7d9f8c-2xk", "namespace": "shop", "uid": "u1",
                         "creationTimestamp": "2026-09-07T09:00:00Z",
                         "labels": {"app": "api"}},
            "spec": {"nodeName": "node-1",
                     "containers": [{"name": "api", "image": "ghcr.io/x/api:1.4"}]},
            "status": {"phase": "Running", "podIP": "10.244.1.7",
                       "containerStatuses": [{"ready": true, "restartCount": 3,
                                              "state": {"running": {}}}]}
        }))
    }

    #[test]
    fn every_kind_has_as_many_cells_as_columns() {
        // The property the whole design exists for: titles and cells come
        // from one list, so they cannot disagree.
        let kinds = [
            "Pod",
            "Deployment",
            "StatefulSet",
            "ReplicaSet",
            "DaemonSet",
            "Job",
            "CronJob",
            "Node",
            "Namespace",
            "Service",
            "Endpoints",
            "Ingress",
            "ConfigMap",
            "Secret",
            "PersistentVolumeClaim",
            "PersistentVolume",
            "StorageClass",
            "ServiceAccount",
            "HorizontalPodAutoscaler",
            "RoleBinding",
            "Event",
            "Rollout",
        ];
        let empty = object(json!({"metadata": {"name": "x", "namespace": "n"}}));
        for kind in kinds {
            for show_namespace in [false, true] {
                let columns = ColumnSet::for_kind(kind, true, show_namespace);
                assert_eq!(
                    columns.cells(&empty, now()).len(),
                    columns.len(),
                    "{kind}, namespace column {show_namespace}"
                );
            }
        }
    }

    #[test]
    fn every_kind_starts_with_a_name_and_ends_with_an_age() {
        for kind in ["Pod", "Node", "Rollout", "Event"] {
            let columns = ColumnSet::for_kind(kind, true, true);
            let titles: Vec<&str> = columns
                .columns()
                .map(|column| column.title.as_str())
                .collect();
            assert_eq!(titles.first(), Some(&"NAME"), "{kind}");
            assert_eq!(titles.last(), Some(&"AGE"), "{kind}");
        }
    }

    #[test]
    fn a_kind_nobody_wrote_columns_for_still_gets_a_table() {
        let columns = ColumnSet::for_kind("Rollout", true, true);
        let titles: Vec<&str> = columns
            .columns()
            .map(|column| column.title.as_str())
            .collect();
        assert_eq!(titles, vec!["NAME", "NAMESPACE", "AGE"]);
    }

    #[test]
    fn the_namespace_column_only_appears_when_it_earns_its_width() {
        assert!(
            !ColumnSet::for_kind("Pod", true, false)
                .columns()
                .any(|column| column.title == "NAMESPACE")
        );
        assert!(
            ColumnSet::for_kind("Pod", true, true)
                .columns()
                .any(|column| column.title == "NAMESPACE")
        );
        // A cluster-scoped kind never gets one, however the table is scoped.
        assert!(
            !ColumnSet::for_kind("Node", false, true)
                .columns()
                .any(|column| column.title == "NAMESPACE")
        );
    }

    #[test]
    fn a_pod_row_is_the_one_kubectl_prints() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let cells = columns.cells(&pod(), now());
        assert_eq!(
            cells,
            vec![
                "api-7d9f8c-2xk",
                "1/1",
                "Running",
                "3",
                "10.244.1.7",
                "node-1",
                "3h"
            ]
        );
    }

    #[test]
    fn a_row_carries_its_health_and_a_haystack_the_filter_can_match_labels_in() {
        let row = ColumnSet::for_kind("Pod", true, false).row(&pod(), now());
        assert_eq!(row.key, "u1");
        assert_eq!(row.namespace.as_deref(), Some("shop"));
        assert_eq!(row.health.level, Level::Ok);
        assert!(row.haystack.contains("app=api"));
        assert!(row.haystack.contains("node-1"));
        assert!(!row.is_bad());
    }

    #[test]
    fn a_row_without_a_uid_is_keyed_by_where_it_lives() {
        let anonymous = object(json!({"metadata": {"name": "a", "namespace": "n"}}));
        assert_eq!(row_key(&anonymous), "n/a");
        let cluster_scoped = object(json!({"metadata": {"name": "node-1"}}));
        assert_eq!(row_key(&cluster_scoped), "node-1");
    }

    #[test]
    fn an_absent_field_reads_as_kubectl_writes_it() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let unscheduled = object(json!({
            "metadata": {"name": "p"},
            "spec": {"containers": [{"name": "c"}]},
            "status": {"phase": "Pending"}
        }));
        let cells = columns.cells(&unscheduled, now());
        assert!(cells.contains(&NONE.to_string()), "{cells:?}");
    }

    #[test]
    fn a_service_says_pending_while_its_load_balancer_has_no_address() {
        let columns = ColumnSet::for_kind("Service", true, false);
        let pending = object(json!({
            "metadata": {"name": "web"},
            "spec": {"type": "LoadBalancer", "clusterIP": "10.96.0.9",
                     "ports": [{"port": 443, "protocol": "TCP"}]},
            "status": {}
        }));
        let cells = columns.cells(&pending, now());
        assert!(cells.contains(&"<pending>".to_string()), "{cells:?}");
        assert!(cells.contains(&"443/TCP".to_string()), "{cells:?}");

        let assigned = object(json!({
            "metadata": {"name": "web"},
            "spec": {"type": "LoadBalancer", "ports": [{"port": 443, "nodePort": 31000}]},
            "status": {"loadBalancer": {"ingress": [{"hostname": "a.elb.example"}]}}
        }));
        let cells = columns.cells(&assigned, now());
        assert!(cells.contains(&"a.elb.example".to_string()), "{cells:?}");
        assert!(cells.contains(&"443:31000/TCP".to_string()), "{cells:?}");
    }

    #[test]
    fn a_nodes_roles_come_from_its_labels_and_sort() {
        let node = object(json!({"metadata": {"name": "n", "labels": {
            "node-role.kubernetes.io/worker": "",
            "node-role.kubernetes.io/control-plane": "",
            "kubernetes.io/os": "linux"
        }}}));
        assert_eq!(node_roles(&node), "control-plane,worker");
        let plain = object(json!({"metadata": {"name": "n"}}));
        assert_eq!(node_roles(&plain), NONE);
    }

    #[test]
    fn images_come_from_a_controllers_template_when_it_has_one() {
        let deployment = object(json!({
            "metadata": {"name": "api"},
            "spec": {"template": {"spec": {"containers": [
                {"name": "api", "image": "ghcr.io/x/api:1.4"},
                {"name": "proxy", "image": "envoy:1.31"}
            ]}}}
        }));
        assert_eq!(images(&deployment), "ghcr.io/x/api:1.4,envoy:1.31");
        assert_eq!(images(&pod()), "ghcr.io/x/api:1.4");
    }

    #[test]
    fn a_cron_job_prints_its_suspension_the_way_kubectl_does() {
        let columns = ColumnSet::for_kind("CronJob", true, false);
        let suspended = object(json!({
            "metadata": {"name": "reindex"},
            "spec": {"schedule": "*/15 * * * *", "suspend": true},
            "status": {}
        }));
        let cells = columns.cells(&suspended, now());
        assert!(cells.contains(&"True".to_string()), "{cells:?}");
        assert!(cells.contains(&"*/15 * * * *".to_string()), "{cells:?}");
        // Never scheduled, so there is no last schedule.
        assert!(cells.contains(&NONE.to_string()), "{cells:?}");
    }

    #[test]
    fn the_kinds_that_churn_sort_newest_first() {
        for kind in ["Pod", "Event", "Job"] {
            let (column, ascending) = ColumnSet::for_kind(kind, true, false).default_sort();
            assert!(
                ColumnSet::for_kind(kind, true, false).is_age(column),
                "{kind} should sort by age"
            );
            assert!(ascending, "{kind} should sort youngest first");
        }
        let (column, ascending) = ColumnSet::for_kind("ConfigMap", true, false).default_sort();
        assert_eq!(column, 0);
        assert!(ascending);
    }

    #[test]
    fn sorting_by_age_uses_the_timestamp_and_not_the_string() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let make = |name: &str, created: &str| {
            columns.row(
                &object(json!({"metadata": {"name": name, "creationTimestamp": created}})),
                now(),
            )
        };
        // `9m59s` and `10m` compare the wrong way round as text.
        let mut rows = vec![
            make("older", "2026-09-07T11:50:00Z"),
            make("newer", "2026-09-07T11:50:01Z"),
        ];
        let (age, _) = columns.default_sort();
        sort(&mut rows, &columns, age, true);
        assert_eq!(rows[0].name, "newer");
        sort(&mut rows, &columns, age, false);
        assert_eq!(rows[0].name, "older");
    }

    #[test]
    fn sorting_a_number_column_compares_numbers() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let make = |name: &str, restarts: i64| {
            columns.row(
                &object(json!({
                    "metadata": {"name": name},
                    "status": {"phase": "Running",
                               "containerStatuses": [{"ready": true, "restartCount": restarts}]}
                })),
                now(),
            )
        };
        let restarts = columns
            .columns()
            .position(|column| column.title == "RESTARTS")
            .unwrap();
        let mut rows = vec![make("nine", 9), make("ten", 10), make("two", 2)];
        sort(&mut rows, &columns, restarts, true);
        let order: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(order, vec!["two", "nine", "ten"]);
    }

    #[test]
    fn a_row_whose_object_has_not_moved_is_reused_rather_than_reformatted() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let object = object(json!({
            "metadata": {"name": "api", "uid": "u1", "resourceVersion": "10"},
            "status": {"phase": "Running", "containerStatuses": [{"ready": true}]}
        }));
        // A doctored row proves reuse: nothing but carrying it over could
        // produce this cell.
        let mut previous = columns.row(&object, now());
        previous.cells[0] = "carried over".into();

        let rows = columns.rows(std::slice::from_ref(&object), &[previous], now());
        assert_eq!(rows[0].cells[0], "carried over");
    }

    #[test]
    fn a_row_whose_object_changed_is_built_again() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let before = object(json!({
            "metadata": {"name": "api", "uid": "u1", "resourceVersion": "10"},
            "status": {"phase": "Running", "containerStatuses": [{"ready": true}]}
        }));
        let mut stale = columns.row(&before, now());
        stale.cells[0] = "carried over".into();

        let after = object(json!({
            "metadata": {"name": "api", "uid": "u1", "resourceVersion": "11"},
            "status": {"phase": "Running", "containerStatuses": [
                {"ready": false, "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
            ]}
        }));
        let rows = columns.rows(&[after], &[stale], now());
        assert_eq!(rows[0].cells[0], "api");
        assert_eq!(rows[0].health.level, Level::Error);
        assert_eq!(rows[0].version, "11");
    }

    #[test]
    fn rows_come_back_in_the_lists_order_however_the_previous_ones_were_sorted() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let make = |name: &str, uid: &str| {
            object(json!({"metadata": {"name": name, "uid": uid, "resourceVersion": "1"}}))
        };
        let objects = vec![make("a", "1"), make("b", "2"), make("c", "3")];
        let mut previous: Vec<Row> = objects.iter().map(|o| columns.row(o, now())).collect();
        previous.reverse();
        // And one row for something that has since gone away.
        previous.push(columns.row(&make("ghost", "9"), now()));

        let rows = columns.rows(&objects, &previous, now());
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn a_recreated_object_does_not_inherit_the_old_ones_row() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let old = object(json!({
            "metadata": {"name": "api", "uid": "old", "resourceVersion": "10"}
        }));
        let mut stale = columns.row(&old, now());
        stale.cells[0] = "carried over".into();
        // Same name, same version, different uid: a different object.
        let new = object(json!({
            "metadata": {"name": "api", "uid": "new", "resourceVersion": "10"}
        }));
        let rows = columns.rows(&[new], &[stale], now());
        assert_eq!(rows[0].cells[0], "api");
    }

    #[test]
    fn building_rows_with_nothing_to_reuse_is_the_same_as_building_them_one_by_one() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let objects = vec![pod()];
        assert_eq!(
            columns.rows(&objects, &[], now()),
            vec![columns.row(&objects[0], now())]
        );
    }

    fn printer(name: &str, kind: &str, path: &str) -> PrinterColumn {
        PrinterColumn {
            name: name.into(),
            kind: kind.into(),
            json_path: path.into(),
            priority: 0,
        }
    }

    fn widgets() -> ApiResource {
        ApiResource {
            group: "example.kirikumo.dev".into(),
            version: "v1".into(),
            kind: "Widget".into(),
            name: "widgets".into(),
            singular: "widget".into(),
            namespaced: true,
            verbs: vec!["list".into()],
            short_names: Vec::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn a_custom_resource_gets_the_columns_its_crd_declares_between_name_and_age() {
        let columns = ColumnSet::for_resource(
            &widgets(),
            false,
            Some(&[
                printer("Colour", "string", ".spec.colour"),
                printer("Replicas", "integer", ".spec.replicas"),
                printer(
                    "Ready",
                    "string",
                    ".status.conditions[?(@.type==\"Ready\")].status",
                ),
                printer("Started", "date", ".metadata.creationTimestamp"),
            ]),
        );
        let titles: Vec<&str> = columns
            .columns()
            .map(|column| column.title.as_str())
            .collect();
        assert_eq!(
            titles,
            vec!["NAME", "COLOUR", "REPLICAS", "READY", "STARTED", "AGE"]
        );
        let widget = object(json!({
            "metadata": {"name": "red-one", "namespace": "shop",
                         "creationTimestamp": "2026-09-07T09:00:00Z"},
            "spec": {"colour": "red", "replicas": 3},
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}
        }));
        assert_eq!(
            columns.cells(&widget, now()),
            vec!["red-one", "red", "3", "True", "3h", "3h"]
        );
        // A numeric printer column is drawn as a number.
        assert!(columns.columns().nth(2).unwrap().numeric);
    }

    #[test]
    fn an_argo_application_row_uses_its_api_group_for_health() {
        let resource = ApiResource {
            group: "argoproj.io".into(),
            version: "v1alpha1".into(),
            kind: "Application".into(),
            name: "applications".into(),
            ..widgets()
        };
        let columns = ColumnSet::for_resource(&resource, false, None);
        let application = object(json!({
            "metadata": {"name": "shop"},
            "status": {"sync": {"status": "OutOfSync"},
                       "health": {"status": "Healthy"}}
        }));
        assert_eq!(
            columns.row(&application, now()).health,
            Health::new(Level::Attention, "OutOfSync")
        );
    }

    #[test]
    fn a_kind_with_columns_of_its_own_keeps_them_whatever_a_crd_says() {
        // Not that a Pod has a CRD; the rule is what matters: the hand-written
        // set wins, so a printer column can never displace READY.
        let columns = ColumnSet::for_resource(
            &ApiResource {
                kind: "Pod".into(),
                name: "pods".into(),
                ..widgets()
            },
            false,
            Some(&[printer("X", "string", ".x")]),
        );
        let titles: Vec<&str> = columns
            .columns()
            .map(|column| column.title.as_str())
            .collect();
        assert!(titles.contains(&"READY"));
        assert!(!titles.contains(&"X"));
    }

    #[test]
    fn a_crd_with_no_columns_declared_gets_the_plain_table() {
        let columns = ColumnSet::for_resource(&widgets(), true, Some(&[]));
        let titles: Vec<&str> = columns
            .columns()
            .map(|column| column.title.as_str())
            .collect();
        assert_eq!(titles, vec!["NAME", "NAMESPACE", "AGE"]);
    }

    #[test]
    fn a_printer_column_the_object_does_not_have_is_an_empty_cell() {
        let columns = ColumnSet::for_resource(
            &widgets(),
            false,
            Some(&[
                printer("Phase", "string", ".status.phase"),
                printer("Started", "date", ".status.startedAt"),
            ]),
        );
        let bare = object(json!({"metadata": {"name": "w"}}));
        assert_eq!(
            columns.cells(&bare, now()),
            vec!["w", "", "<none>", "<unknown>"]
        );
    }

    #[test]
    fn a_table_reports_what_is_worth_looking_at() {
        let columns = ColumnSet::for_kind("Pod", true, false);
        let healthy = columns.row(&pod(), now());
        let broken = columns.row(
            &object(json!({
                "metadata": {"name": "b"},
                "status": {"phase": "Running", "containerStatuses": [
                    {"ready": false, "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
                ]}
            })),
            now(),
        );
        let rows = vec![healthy, broken];
        assert_eq!(bad_rows(&rows), 1);
        assert_eq!(worst(&rows), Level::Error);
        assert_eq!(worst(&[]), Level::Unknown);
    }

    #[test]
    fn a_share_of_nothing_is_not_zero_percent() {
        assert_eq!(percent(50, 100), Some(50.0));
        assert_eq!(percent(1, 0), None);
    }

    #[test]
    fn a_capacity_is_shown_as_the_manifest_wrote_it() {
        let columns = ColumnSet::for_kind("PersistentVolumeClaim", true, false);
        let claim = object(json!({
            "metadata": {"name": "data"},
            "spec": {"storageClassName": "standard"},
            "status": {"phase": "Bound", "capacity": {"storage": "50Gi"}}
        }));
        assert!(columns.cells(&claim, now()).contains(&"50Gi".to_string()));
        assert_eq!(as_bytes("50Gi").as_deref(), Some("50Gi"));
    }
}
