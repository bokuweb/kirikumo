//! Native presentation data for GitOps controllers.
//!
//! Argo CD already persists the useful read-side state in its `Application`
//! custom resource. Reading that JSON keeps this integration on the normal
//! [`kirikumo_kube::Cluster`] path: no Argo session, token, port-forward or
//! second client is needed. The managed resources are parsed here even though
//! the first view only summarizes them, so a later virtualized tree does not
//! have to put domain decisions in `kirikumo-views`. Its row order and marks
//! are view-model decisions here and are covered before the GPUI list uses them.

use kirikumo_kube::{Object, ResourceKey};
use serde_json::Value;

/// The Argo CD API group.
pub const ARGO_CD_GROUP: &str = "argoproj.io";

/// The read-side state of one Argo CD `Application`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    /// The Argo CD project which admits the application.
    pub project: String,
    /// `Synced`, `OutOfSync` or `Unknown`.
    pub sync: String,
    /// `Healthy`, `Degraded`, `Progressing` or another Argo health word.
    pub health: String,
    /// The current operation phase, when a sync or rollback is in flight.
    pub operation: String,
    /// The revision Argo last compared with the destination.
    pub revision: String,
    /// Where the rendered resources are applied.
    pub destination: String,
    /// Argo CD's name for the destination cluster, when configured.
    ///
    /// This is display metadata, not a kubeconfig context identity.
    pub destination_name: String,
    /// The Kubernetes API server Argo CD applies to.
    pub destination_server: String,
    /// The namespace Argo CD uses when a manifest does not name one.
    pub destination_namespace: Option<String>,
    /// Whether automated sync is enabled, including its important options.
    pub sync_policy: String,
    /// Git or chart sources, in manifest order.
    pub sources: Vec<Source>,
    /// The resources Argo reports for the application.
    pub resources: Vec<ManagedResource>,
}

impl Application {
    /// Read an Argo CD Application from its unstructured Kubernetes object.
    pub fn from_object(object: &Object) -> Self {
        let sources = match object.at("spec.sources").and_then(Value::as_array) {
            Some(sources) if !sources.is_empty() => {
                sources.iter().map(Source::from_value).collect()
            }
            _ => object
                .at("spec.source")
                .filter(|source| source.is_object())
                .map(Source::from_value)
                .into_iter()
                .collect(),
        };
        let resources = object
            .array_at("status.resources")
            .iter()
            .filter_map(ManagedResource::from_value)
            .collect();
        let destination_name = object.str_at("spec.destination.name").to_string();
        let destination_server = object.str_at("spec.destination.server").to_string();
        let destination_namespace = object
            .at("spec.destination.namespace")
            .and_then(Value::as_str)
            .filter(|namespace| !namespace.is_empty())
            .map(str::to_string);
        Self {
            project: object.str_at("spec.project").to_string(),
            sync: object.str_at("status.sync.status").to_string(),
            health: object.str_at("status.health.status").to_string(),
            operation: object.str_at("status.operationState.phase").to_string(),
            revision: object.str_at("status.sync.revision").to_string(),
            destination: destination(object),
            destination_name,
            destination_server,
            destination_namespace,
            sync_policy: sync_policy(object),
            sources,
            resources,
        }
    }

    /// A compact count for the Overview. The full collection is kept for a
    /// virtualized resource tree; it must not become one element per resource
    /// in the non-virtualized Overview.
    pub fn resources_summary(&self) -> String {
        let out_of_sync = self
            .resources
            .iter()
            .filter(|resource| resource.sync == "OutOfSync")
            .count();
        let unhealthy = self
            .resources
            .iter()
            .filter(|resource| {
                !resource.health.is_empty()
                    && !matches!(resource.health.as_str(), "Healthy" | "Progressing")
            })
            .count();
        let mut parts = vec![format!("{} resources", self.resources.len())];
        if out_of_sync > 0 {
            parts.push(format!("{out_of_sync} out of sync"));
        }
        if unhealthy > 0 {
            parts.push(format!("{unhealthy} unhealthy"));
        }
        parts.join(" · ")
    }

    /// Managed resources in the stable order the Resources tab scans.
    ///
    /// Argo's status order is an implementation detail and can move between
    /// reconciliations. A namespace/kind/name order keeps a watch update from
    /// shuffling unrelated rows under the reader.
    pub fn resources_for_display(&self) -> Vec<ManagedResource> {
        let mut resources = self.resources.clone();
        resources.sort_by(|left, right| {
            left.namespace
                .as_deref()
                .unwrap_or_default()
                .cmp(right.namespace.as_deref().unwrap_or_default())
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.group.cmp(&right.group))
        });
        resources
    }
}

/// The read-side state of one Argo CD `ApplicationSet`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationSet {
    /// The project copied into generated Applications.
    pub project: String,
    /// Generator kinds, in manifest order.
    pub generators: Vec<String>,
    /// The destination template shown as cluster and namespace.
    pub destination: String,
    /// Whether the template uses Go template evaluation.
    pub go_template: bool,
    /// Progressive-sync strategy, when configured.
    pub strategy: String,
    /// Total Applications managed, including entries omitted from a bounded
    /// `status.resources` collection.
    pub applications: usize,
    /// The health word calculated by newer ApplicationSet controllers.
    pub health: String,
}

impl ApplicationSet {
    /// Read an Argo CD ApplicationSet from its unstructured Kubernetes object.
    pub fn from_object(object: &Object) -> Self {
        let generators = object
            .array_at("spec.generators")
            .iter()
            .filter_map(|generator| {
                generator.as_object()?.iter().find_map(|(name, value)| {
                    (!matches!(name.as_str(), "selector" | "template") && !value.is_null())
                        .then(|| name.clone())
                })
            })
            .collect();
        let reported = object
            .at("status.resourcesCount")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or_default();
        let observed = object.array_at("status.resources").len();
        Self {
            project: object.str_at("spec.template.spec.project").to_string(),
            generators,
            destination: destination_from(
                object,
                "spec.template.spec.destination.name",
                "spec.template.spec.destination.server",
                "spec.template.spec.destination.namespace",
            ),
            go_template: object
                .at("spec.goTemplate")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            strategy: object.str_at("spec.strategy.type").to_string(),
            applications: reported.max(observed),
            health: object.str_at("status.health.status").to_string(),
        }
    }
}

/// The deployment and access boundaries of one Argo CD `AppProject`.
///
/// Role names are presentation state; JWT token material is deliberately not
/// retained here and therefore cannot accidentally reach the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppProject {
    /// Human description of the project.
    pub description: String,
    /// Repository patterns Applications may use.
    pub source_repositories: Vec<String>,
    /// Namespaces Applications may be created in.
    pub source_namespaces: Vec<String>,
    /// Cluster and namespace patterns Applications may target.
    pub destinations: Vec<String>,
    /// Allowed cluster-scoped group/kinds.
    pub cluster_allow: Vec<String>,
    /// Denied cluster-scoped group/kinds.
    pub cluster_deny: Vec<String>,
    /// Allowed namespace-scoped group/kinds.
    pub namespace_allow: Vec<String>,
    /// Denied namespace-scoped group/kinds.
    pub namespace_deny: Vec<String>,
    /// RBAC role names, without their tokens.
    pub roles: Vec<String>,
    /// Orphan monitoring mode and ignored-pattern count.
    pub orphaned_resources: String,
    /// Whether destinations must use clusters scoped to this project.
    pub project_scoped_clusters_only: bool,
    /// Number of configured sync windows.
    pub sync_windows: usize,
}

impl AppProject {
    /// Read an Argo CD AppProject from its unstructured Kubernetes object.
    pub fn from_object(object: &Object) -> Self {
        let destinations = object
            .array_at("spec.destinations")
            .iter()
            .map(destination_value)
            .filter(|destination| !destination.is_empty())
            .collect();
        let roles = object
            .array_at("spec.roles")
            .iter()
            .filter_map(|role| role.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let orphaned_resources = object
            .at("spec.orphanedResources")
            .filter(|orphaned| orphaned.is_object())
            .map(|orphaned| {
                let mode = match orphaned.get("warn").and_then(Value::as_bool) {
                    Some(true) => "Warnings enabled",
                    _ => "Warnings disabled",
                };
                let ignored = orphaned
                    .get("ignore")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or_default();
                format!("{mode} · {ignored} ignored")
            })
            .unwrap_or_default();
        Self {
            description: object.str_at("spec.description").to_string(),
            source_repositories: strings_at(object, "spec.sourceRepos"),
            source_namespaces: strings_at(object, "spec.sourceNamespaces"),
            destinations,
            cluster_allow: group_kinds_at(object, "spec.clusterResourceWhitelist"),
            cluster_deny: group_kinds_at(object, "spec.clusterResourceBlacklist"),
            namespace_allow: group_kinds_at(object, "spec.namespaceResourceWhitelist"),
            namespace_deny: group_kinds_at(object, "spec.namespaceResourceBlacklist"),
            roles,
            orphaned_resources,
            project_scoped_clusters_only: object
                .at("spec.permitOnlyProjectScopedClusters")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            sync_windows: object.array_at("spec.syncWindows").len(),
        }
    }
}

/// One Git repository or Helm source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Git or Helm repository URL.
    pub repository: String,
    /// A directory in a Git repository.
    pub path: String,
    /// A Helm chart name.
    pub chart: String,
    /// Branch, tag, semver range or commit requested by the spec.
    pub target_revision: String,
}

impl Source {
    fn from_value(source: &Value) -> Self {
        Self {
            repository: string(source, "repoURL"),
            path: string(source, "path"),
            chart: string(source, "chart"),
            target_revision: string(source, "targetRevision"),
        }
    }
}

/// One live resource Argo CD associates with an Application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedResource {
    /// Kubernetes API group; empty for core resources.
    pub group: String,
    /// Kubernetes kind.
    pub kind: String,
    /// Namespace, if the resource is namespaced.
    pub namespace: Option<String>,
    /// Object name.
    pub name: String,
    /// Argo CD sync word for this resource.
    pub sync: String,
    /// Argo CD health word for this resource, when it has one.
    pub health: String,
}

impl ManagedResource {
    fn from_value(resource: &Value) -> Option<Self> {
        let kind = resource.get("kind")?.as_str()?.to_string();
        let name = resource.get("name")?.as_str()?.to_string();
        Some(Self {
            group: string(resource, "group"),
            kind,
            namespace: resource
                .get("namespace")
                .and_then(Value::as_str)
                .filter(|namespace| !namespace.is_empty())
                .map(str::to_string),
            name,
            sync: string(resource, "status"),
            health: resource
                .pointer("/health/status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// The catalogue identity used when this resource can be opened in the
    /// cluster Kirikumo currently has selected.
    pub fn key(&self) -> ResourceKey {
        ResourceKey::new(self.group.clone(), self.kind.clone())
    }

    /// The health mark for this row.
    ///
    /// A broken or progressing resource outranks its sync word. Otherwise an
    /// out-of-sync resource is worth attention even when it is still serving
    /// healthy traffic.
    pub fn level(&self) -> kirikumo_kube::Level {
        use kirikumo_kube::Level;
        match self.health.as_str() {
            "Degraded" | "Missing" => return Level::Error,
            "Progressing" => return Level::Working,
            "Suspended" => return Level::Attention,
            _ => {}
        }
        match self.sync.as_str() {
            "OutOfSync" => Level::Attention,
            "Synced" => Level::Ok,
            _ => Level::Unknown,
        }
    }

    /// Navigate to this object only when Argo's destination is provably the
    /// cluster currently connected to Kirikumo.
    ///
    /// An Argo destination name is deliberately insufficient: it belongs to
    /// Argo's cluster registry, not to kubeconfig, and equal-looking aliases
    /// can name different clusters. `namespaced` is supplied by discovery;
    /// `None` means the kind is not served here and must not become a link.
    pub fn target(
        &self,
        application: &Application,
        current_server: Option<&str>,
        namespaced: Option<bool>,
    ) -> Option<crate::detail::Target> {
        let current_server = current_server?.trim_end_matches('/');
        let destination_server = application.destination_server.trim_end_matches('/');
        if destination_server.is_empty() || destination_server != current_server {
            return None;
        }
        let namespace = match namespaced? {
            true => self
                .namespace
                .clone()
                .or_else(|| application.destination_namespace.clone()),
            false => None,
        };
        Some(crate::detail::Target::Object {
            key: self.key(),
            namespace,
            name: self.name.clone(),
        })
    }
}

fn destination(object: &Object) -> String {
    destination_from(
        object,
        "spec.destination.name",
        "spec.destination.server",
        "spec.destination.namespace",
    )
}

fn destination_from(
    object: &Object,
    name_path: &str,
    server_path: &str,
    namespace_path: &str,
) -> String {
    let cluster = [object.str_at(name_path), object.str_at(server_path)]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let namespace = object.str_at(namespace_path);
    match (cluster.is_empty(), namespace.is_empty()) {
        (false, false) => format!("{cluster} · {namespace}"),
        (false, true) => cluster.to_string(),
        (true, false) => namespace.to_string(),
        (true, true) => String::new(),
    }
}

fn destination_value(destination: &Value) -> String {
    let cluster = [string(destination, "name"), string(destination, "server")]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let namespace = string(destination, "namespace");
    match (cluster.is_empty(), namespace.is_empty()) {
        (false, false) => format!("{cluster} · {namespace}"),
        (false, true) => cluster,
        (true, false) => namespace,
        (true, true) => String::new(),
    }
}

fn strings_at(object: &Object, path: &str) -> Vec<String> {
    object
        .array_at(path)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn group_kinds_at(object: &Object, path: &str) -> Vec<String> {
    object
        .array_at(path)
        .iter()
        .filter_map(|resource| {
            let kind = resource.get("kind")?.as_str()?;
            let group = resource
                .get("group")
                .and_then(Value::as_str)
                .filter(|group| !group.is_empty())
                .unwrap_or("core");
            Some(format!("{group}/{kind}"))
        })
        .collect()
}

fn sync_policy(object: &Object) -> String {
    let Some(automated) = object.at("spec.syncPolicy.automated") else {
        return "Manual".into();
    };
    let mut parts = vec!["Automated".to_string()];
    if automated.get("prune").and_then(Value::as_bool) == Some(true) {
        parts.push("prune".into());
    }
    if automated.get("selfHeal").and_then(Value::as_bool) == Some(true) {
        parts.push("self-heal".into());
    }
    parts.join(" · ")
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_application_is_read_without_an_argo_api_session() {
        let object = Object::new(json!({
            "metadata": {"name": "shop", "namespace": "argocd"},
            "spec": {
                "project": "production",
                "destination": {"name": "prod", "namespace": "shop"},
                "source": {"repoURL": "https://github.com/acme/shop", "path": "deploy",
                           "targetRevision": "main"},
                "syncPolicy": {"automated": {"prune": true, "selfHeal": true}}
            },
            "status": {
                "sync": {"status": "OutOfSync", "revision": "abc123"},
                "health": {"status": "Degraded"},
                "operationState": {"phase": "Failed"},
                "resources": [
                    {"group": "apps", "kind": "Deployment", "namespace": "shop",
                     "name": "api", "status": "OutOfSync",
                     "health": {"status": "Degraded"}},
                    {"kind": "Service", "namespace": "shop", "name": "api",
                     "status": "Synced", "health": {"status": "Healthy"}}
                ]
            }
        }))
        .unwrap();
        let application = Application::from_object(&object);
        assert_eq!(application.project, "production");
        assert_eq!(application.destination, "prod · shop");
        assert_eq!(application.sync_policy, "Automated · prune · self-heal");
        assert_eq!(application.sources[0].path, "deploy");
        assert_eq!(
            application.resources[0].key(),
            ResourceKey::new("apps", "Deployment")
        );
        assert_eq!(
            application.resources_summary(),
            "2 resources · 1 out of sync · 1 unhealthy"
        );
    }

    #[test]
    fn multiple_sources_and_a_manual_policy_are_preserved() {
        let object = Object::new(json!({
            "metadata": {"name": "platform"},
            "spec": {"sources": [
                {"repoURL": "https://example/a", "path": "base", "targetRevision": "main"},
                {"repoURL": "https://example/charts", "chart": "api", "targetRevision": "1.*"}
            ]}
        }))
        .unwrap();
        let application = Application::from_object(&object);
        assert_eq!(application.sources.len(), 2);
        assert_eq!(application.sources[1].chart, "api");
        assert_eq!(application.sync_policy, "Manual");
    }

    #[test]
    fn resources_are_sorted_for_scanning_by_namespace_kind_and_name() {
        let object = Object::new(json!({
            "metadata": {"name": "shop"},
            "status": {"resources": [
                {"kind": "Service", "namespace": "shop", "name": "web"},
                {"group": "apps", "kind": "Deployment", "namespace": "shop", "name": "web"},
                {"kind": "Namespace", "name": "shop"},
                {"group": "apps", "kind": "Deployment", "namespace": "admin", "name": "api"}
            ]}
        }))
        .unwrap();
        let resources = Application::from_object(&object).resources_for_display();
        let order: Vec<(&str, &str, &str)> = resources
            .iter()
            .map(|resource| {
                (
                    resource.namespace.as_deref().unwrap_or_default(),
                    resource.kind.as_str(),
                    resource.name.as_str(),
                )
            })
            .collect();
        assert_eq!(
            order,
            vec![
                ("", "Namespace", "shop"),
                ("admin", "Deployment", "api"),
                ("shop", "Deployment", "web"),
                ("shop", "Service", "web"),
            ]
        );
    }

    #[test]
    fn a_managed_resources_mark_prefers_health_then_sync() {
        let make = |sync: &str, health: &str| ManagedResource {
            group: "apps".into(),
            kind: "Deployment".into(),
            namespace: Some("shop".into()),
            name: "api".into(),
            sync: sync.into(),
            health: health.into(),
        };
        assert_eq!(make("Synced", "Healthy").level(), kirikumo_kube::Level::Ok);
        assert_eq!(
            make("OutOfSync", "Healthy").level(),
            kirikumo_kube::Level::Attention
        );
        assert_eq!(
            make("Synced", "Progressing").level(),
            kirikumo_kube::Level::Working
        );
        assert_eq!(
            make("Synced", "Degraded").level(),
            kirikumo_kube::Level::Error
        );
    }

    #[test]
    fn a_resource_only_links_when_the_destination_server_is_the_current_cluster() {
        let application = Application::from_object(
            &Object::new(json!({
                "metadata": {"name": "shop"},
                "spec": {"destination": {
                    "name": "production",
                    "server": "https://prod.example.test/",
                    "namespace": "shop"
                }}
            }))
            .unwrap(),
        );
        let resource = ManagedResource {
            group: "apps".into(),
            kind: "Deployment".into(),
            namespace: Some("shop".into()),
            name: "api".into(),
            sync: "Synced".into(),
            health: "Healthy".into(),
        };

        assert_eq!(
            resource.target(&application, Some("https://prod.example.test"), Some(true)),
            Some(crate::detail::Target::Object {
                key: ResourceKey::new("apps", "Deployment"),
                namespace: Some("shop".into()),
                name: "api".into(),
            })
        );
        assert_eq!(
            resource.target(&application, Some("https://dev.example.test"), Some(true)),
            None
        );
        assert_eq!(resource.target(&application, None, Some(true)), None);
    }

    #[test]
    fn an_argo_destination_name_alone_is_not_proof_of_the_current_cluster() {
        let application = Application::from_object(
            &Object::new(json!({
                "metadata": {"name": "shop"},
                "spec": {"destination": {"name": "kind-dev", "namespace": "shop"}}
            }))
            .unwrap(),
        );
        let resource = ManagedResource {
            group: "".into(),
            kind: "Service".into(),
            namespace: Some("shop".into()),
            name: "api".into(),
            sync: "Synced".into(),
            health: "Healthy".into(),
        };

        assert_eq!(
            resource.target(
                &application,
                Some("https://kind-dev.example.test"),
                Some(true)
            ),
            None
        );
    }

    #[test]
    fn a_resource_only_links_after_discovery_and_uses_kind_scope() {
        let application = Application::from_object(
            &Object::new(json!({
                "metadata": {"name": "shop"},
                "spec": {"destination": {
                    "server": "https://prod.example.test",
                    "namespace": "shop"
                }}
            }))
            .unwrap(),
        );
        let resource = ManagedResource {
            group: "".into(),
            kind: "Namespace".into(),
            namespace: Some("incorrect-from-argo".into()),
            name: "shop".into(),
            sync: "Synced".into(),
            health: "Healthy".into(),
        };

        assert_eq!(
            resource.target(&application, Some("https://prod.example.test"), None),
            None
        );
        assert_eq!(
            resource.target(&application, Some("https://prod.example.test"), Some(false)),
            Some(crate::detail::Target::Object {
                key: ResourceKey::new("", "Namespace"),
                namespace: None,
                name: "shop".into(),
            })
        );
    }

    #[test]
    fn an_application_set_preserves_generation_and_template_state() {
        let object = Object::new(json!({
            "metadata": {"name": "environments", "namespace": "argocd"},
            "spec": {
                "goTemplate": true,
                "generators": [
                    {"git": {"repoURL": "https://example/platform"}},
                    {"clusters": {"selector": {}}},
                    {"matrix": {"generators": []}}
                ],
                "template": {"spec": {
                    "project": "platform",
                    "destination": {"server": "https://kubernetes.default.svc",
                                    "namespace": "{{.environment}}"}
                }},
                "strategy": {"type": "RollingSync"}
            },
            "status": {
                "resourcesCount": 12,
                "resources": [{"name": "one"}, {"name": "two"}],
                "health": {"status": "Progressing"}
            }
        }))
        .unwrap();

        let application_set = ApplicationSet::from_object(&object);
        assert_eq!(application_set.project, "platform");
        assert_eq!(
            application_set.generators,
            vec!["git", "clusters", "matrix"]
        );
        assert_eq!(
            application_set.destination,
            "https://kubernetes.default.svc · {{.environment}}"
        );
        assert!(application_set.go_template);
        assert_eq!(application_set.strategy, "RollingSync");
        assert_eq!(application_set.applications, 12);
        assert_eq!(application_set.health, "Progressing");
    }

    #[test]
    fn an_application_set_falls_back_to_the_resources_it_reports() {
        let object = Object::new(json!({
            "metadata": {"name": "small"},
            "spec": {"generators": [{"list": {"elements": []}}]},
            "status": {"resources": [{"name": "one"}, {"name": "two"}]}
        }))
        .unwrap();

        let application_set = ApplicationSet::from_object(&object);
        assert_eq!(application_set.generators, vec!["list"]);
        assert_eq!(application_set.applications, 2);
        assert!(!application_set.go_template);
    }

    #[test]
    fn an_app_project_preserves_its_deployment_boundaries_without_tokens() {
        let object = Object::new(json!({
            "metadata": {"name": "production", "namespace": "argocd"},
            "spec": {
                "description": "Production workloads",
                "sourceRepos": ["https://github.com/acme/*", "!https://github.com/acme/test"],
                "sourceNamespaces": ["argocd", "platform"],
                "destinations": [
                    {"name": "production", "namespace": "shop-*"},
                    {"server": "https://kubernetes.default.svc", "namespace": "observability"}
                ],
                "clusterResourceWhitelist": [{"group": "", "kind": "Namespace"}],
                "namespaceResourceWhitelist": [{"group": "apps", "kind": "Deployment"},
                                                {"group": "", "kind": "Service"}],
                "clusterResourceBlacklist": [{"group": "rbac.authorization.k8s.io", "kind": "*"}],
                "namespaceResourceBlacklist": [{"group": "", "kind": "Secret"}],
                "roles": [
                    {"name": "read-only", "groups": ["engineering"],
                     "jwtTokens": [{"iat": 123456789}]},
                    {"name": "deploy"}
                ],
                "orphanedResources": {"warn": true, "ignore": [{"kind": "Secret", "name": "generated-*"}]},
                "permitOnlyProjectScopedClusters": true,
                "syncWindows": [{"kind": "deny", "schedule": "0 22 * * *", "duration": "8h"}]
            },
            "status": {"jwtTokensByRole": {"deploy": {"items": [{"id": "secret"}]}}}
        }))
        .unwrap();

        let project = AppProject::from_object(&object);
        assert_eq!(project.description, "Production workloads");
        assert_eq!(project.source_repositories.len(), 2);
        assert_eq!(project.source_namespaces, vec!["argocd", "platform"]);
        assert_eq!(
            project.destinations,
            vec![
                "production · shop-*",
                "https://kubernetes.default.svc · observability"
            ]
        );
        assert_eq!(project.cluster_allow, vec!["core/Namespace"]);
        assert_eq!(
            project.namespace_allow,
            vec!["apps/Deployment", "core/Service"]
        );
        assert_eq!(project.cluster_deny, vec!["rbac.authorization.k8s.io/*"]);
        assert_eq!(project.namespace_deny, vec!["core/Secret"]);
        assert_eq!(project.roles, vec!["read-only", "deploy"]);
        assert_eq!(project.orphaned_resources, "Warnings enabled · 1 ignored");
        assert!(project.project_scoped_clusters_only);
        assert_eq!(project.sync_windows, 1);
    }
}
