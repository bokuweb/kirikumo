//! Asking a cluster what it serves, and sorting the answer into groups.
//!
//! This is what makes a custom resource free (`AGENTS.md` rule 8): the
//! sidebar is built from `/api` and `/apis`, so a cluster running Argo
//! Rollouts gets a Rollouts table without a release here.
//!
//! Discovery is three round trips deep — the group list, then each group's
//! preferred version, then the resources in it — so it happens once per
//! connection and its answer is held for as long as the connection is.

use crate::error::{Error, Result};
use crate::model::{ApiResource, Catalogue, Group, ResourceKey};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// One `APIResourceList` — what `/api/v1` or `/apis/apps/v1` answers.
#[derive(Debug, Deserialize)]
pub struct ResourceListWire {
    #[serde(default, rename = "groupVersion")]
    group_version: String,
    #[serde(default)]
    resources: Vec<ResourceWire>,
}

#[derive(Debug, Deserialize)]
struct ResourceWire {
    name: String,
    #[serde(default, rename = "singularName")]
    singular_name: String,
    #[serde(default)]
    namespaced: bool,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    verbs: Vec<String>,
    #[serde(default, rename = "shortNames")]
    short_names: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
}

impl ResourceListWire {
    /// Parse an `APIResourceList`, with the group/version to attribute it to
    /// when the answer does not carry one (the core group's does not, on some
    /// versions).
    pub fn parse(value: Value, fallback_group_version: &str) -> Result<Vec<ApiResource>> {
        let wire: Self = serde_json::from_value(value)
            .map_err(|error| Error::Malformed(format!("an API resource list: {error}")))?;
        let group_version = match wire.group_version.is_empty() {
            true => fallback_group_version.to_string(),
            false => wire.group_version.clone(),
        };
        let (group, version) = split_group_version(&group_version);
        Ok(wire
            .resources
            .into_iter()
            .filter(|resource| !resource.kind.is_empty())
            .map(|resource| ApiResource {
                group: group.to_string(),
                version: version.to_string(),
                kind: resource.kind,
                name: resource.name,
                singular: resource.singular_name,
                namespaced: resource.namespaced,
                verbs: resource.verbs,
                short_names: resource.short_names,
                categories: resource.categories,
            })
            .collect())
    }
}

/// `apps/v1` into `("apps", "v1")`; `v1` into `("", "v1")`.
pub fn split_group_version(group_version: &str) -> (&str, &str) {
    match group_version.split_once('/') {
        Some((group, version)) => (group, version),
        None => ("", group_version),
    }
}

/// The preferred `groupVersion` of every group the cluster serves.
///
/// `/apis` answers with each group's versions and which one it prefers; a
/// group that names no preference falls back to its first version, because a
/// group with versions and no preferred one is still a group we can list.
pub fn preferred_group_versions(apis: &Value) -> Vec<String> {
    apis.get("groups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| {
                    group
                        .pointer("/preferredVersion/groupVersion")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            group
                                .get("versions")
                                .and_then(Value::as_array)
                                .and_then(|versions| versions.first())
                                .and_then(|version| version.get("groupVersion"))
                                .and_then(Value::as_str)
                        })
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the catalogue from every resource list the cluster answered with.
///
/// Three rules, in this order:
///
/// 1. Only listable kinds get in — a subresource (`pods/log`, `nodes/status`)
///    is not a thing to put in a sidebar.
/// 2. One entry per [`ResourceKey`]: a kind served at two versions appears
///    once, at the version discovery said the group prefers.
/// 3. Core `Event` wins over its `events.k8s.io` projection. Kubernetes serves
///    both APIs for the same records; presenting both would be two identical
///    sidebar rows rather than two resources.
/// 4. The order is the sidebar's: by group, then by the position of the kind
///    in [`ORDER`], then alphabetically for everything that list does not
///    name.
pub fn catalogue(lists: Vec<Vec<ApiResource>>) -> Catalogue {
    let has_core_events = lists.iter().flatten().any(|resource| {
        resource.group.is_empty() && resource.kind == "Event" && resource.is_listable()
    });
    let mut seen: BTreeMap<ResourceKey, ApiResource> = BTreeMap::new();
    for list in lists {
        for resource in list {
            if !resource.is_listable() {
                continue;
            }
            if has_core_events && resource.group == "events.k8s.io" && resource.kind == "Event" {
                continue;
            }
            seen.entry(resource.key()).or_insert(resource);
        }
    }
    let mut resources: Vec<ApiResource> = seen.into_values().collect();
    resources.sort_by(|a, b| {
        let (group_a, group_b) = (group_of(a), group_of(b));
        group_a
            .cmp(&group_b)
            .then_with(|| rank(a).cmp(&rank(b)))
            // Inside Custom Resources the API group is the heading, so kinds
            // of one group have to end up adjacent.
            .then_with(|| a.group.cmp(&b.group))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    Catalogue::new(resources)
}

/// The kinds that have a place of their own in the sidebar, in that order.
///
/// This list is *presentation only*: a kind not on it still appears, under
/// [`Group::Custom`]. What it buys is that a cluster's sidebar looks the same
/// as every other cluster's for the kinds everyone has, which is the whole
/// reason a person can find `Deployments` without reading.
const ORDER: &[(&str, &str, Group)] = &[
    // Cluster
    ("", "Node", Group::Cluster),
    ("", "Namespace", Group::Cluster),
    ("", "Event", Group::Cluster),
    ("events.k8s.io", "Event", Group::Cluster),
    ("", "ComponentStatus", Group::Cluster),
    (
        "apiextensions.k8s.io",
        "CustomResourceDefinition",
        Group::Cluster,
    ),
    // Workloads
    ("", "Pod", Group::Workloads),
    ("apps", "Deployment", Group::Workloads),
    ("apps", "DaemonSet", Group::Workloads),
    ("apps", "StatefulSet", Group::Workloads),
    ("apps", "ReplicaSet", Group::Workloads),
    ("", "ReplicationController", Group::Workloads),
    ("batch", "Job", Group::Workloads),
    ("batch", "CronJob", Group::Workloads),
    // Config
    ("", "ConfigMap", Group::Config),
    ("", "Secret", Group::Config),
    ("", "ResourceQuota", Group::Config),
    ("", "LimitRange", Group::Config),
    ("autoscaling", "HorizontalPodAutoscaler", Group::Config),
    ("policy", "PodDisruptionBudget", Group::Config),
    ("scheduling.k8s.io", "PriorityClass", Group::Config),
    ("node.k8s.io", "RuntimeClass", Group::Config),
    // Network
    ("", "Service", Group::Network),
    ("", "Endpoints", Group::Network),
    ("discovery.k8s.io", "EndpointSlice", Group::Network),
    ("networking.k8s.io", "Ingress", Group::Network),
    ("networking.k8s.io", "IngressClass", Group::Network),
    ("networking.k8s.io", "NetworkPolicy", Group::Network),
    // Storage
    ("", "PersistentVolumeClaim", Group::Storage),
    ("", "PersistentVolume", Group::Storage),
    ("storage.k8s.io", "StorageClass", Group::Storage),
    ("storage.k8s.io", "VolumeAttachment", Group::Storage),
    ("storage.k8s.io", "CSIDriver", Group::Storage),
    ("storage.k8s.io", "CSINode", Group::Storage),
    // Access control
    ("", "ServiceAccount", Group::AccessControl),
    ("rbac.authorization.k8s.io", "Role", Group::AccessControl),
    (
        "rbac.authorization.k8s.io",
        "RoleBinding",
        Group::AccessControl,
    ),
    (
        "rbac.authorization.k8s.io",
        "ClusterRole",
        Group::AccessControl,
    ),
    (
        "rbac.authorization.k8s.io",
        "ClusterRoleBinding",
        Group::AccessControl,
    ),
    // GitOps. These are Argo CD's API, not Argo Rollouts: both use the
    // argoproj.io group, so the kind is part of the distinction.
    ("argoproj.io", "Application", Group::GitOps),
    ("argoproj.io", "ApplicationSet", Group::GitOps),
    ("argoproj.io", "AppProject", Group::GitOps),
];

/// Which sidebar group a kind belongs to.
pub fn group_of(resource: &ApiResource) -> Group {
    ORDER
        .iter()
        .find(|(group, kind, _)| *group == resource.group && *kind == resource.kind)
        .map(|(_, _, group)| *group)
        .unwrap_or(Group::Custom)
}

/// Where a kind sits inside its group. Anything unnamed sorts after
/// everything named, and then alphabetically.
fn rank(resource: &ApiResource) -> usize {
    ORDER
        .iter()
        .position(|(group, kind, _)| *group == resource.group && *kind == resource.kind)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resource(group: &str, version: &str, kind: &str, name: &str, verbs: &[&str]) -> ApiResource {
        ApiResource {
            group: group.into(),
            version: version.into(),
            kind: kind.into(),
            name: name.into(),
            singular: String::new(),
            namespaced: true,
            verbs: verbs.iter().map(|verb| verb.to_string()).collect(),
            short_names: Vec::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn a_resource_list_becomes_resources_attributed_to_its_group() {
        let parsed = ResourceListWire::parse(
            json!({
                "groupVersion": "apps/v1",
                "resources": [
                    {"name": "deployments", "singularName": "deployment", "namespaced": true,
                     "kind": "Deployment", "verbs": ["list", "watch", "delete"],
                     "shortNames": ["deploy"]},
                    {"name": "deployments/status", "namespaced": true,
                     "kind": "Deployment", "verbs": ["get"]}
                ]
            }),
            "",
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].group, "apps");
        assert_eq!(parsed[0].version, "v1");
        assert_eq!(parsed[0].short_names, vec!["deploy"]);
        assert!(parsed[0].is_listable());
        assert!(!parsed[1].is_listable());
    }

    #[test]
    fn the_core_group_has_no_group_name() {
        let parsed = ResourceListWire::parse(
            json!({"groupVersion": "v1", "resources": [
                {"name": "pods", "kind": "Pod", "namespaced": true, "verbs": ["list"]}
            ]}),
            "",
        )
        .unwrap();
        assert_eq!(parsed[0].group, "");
        assert_eq!(parsed[0].api_version(), "v1");
    }

    #[test]
    fn a_list_that_does_not_name_its_group_is_attributed_to_the_one_we_asked_for() {
        let parsed = ResourceListWire::parse(
            json!({"resources": [{"name": "pods", "kind": "Pod", "verbs": ["list"]}]}),
            "v1",
        )
        .unwrap();
        assert_eq!(parsed[0].version, "v1");
    }

    #[test]
    fn the_preferred_version_of_each_group_is_what_we_ask_for() {
        let apis = json!({"groups": [
            {"name": "apps",
             "versions": [{"groupVersion": "apps/v1"}],
             "preferredVersion": {"groupVersion": "apps/v1"}},
            {"name": "autoscaling",
             "versions": [{"groupVersion": "autoscaling/v2"}, {"groupVersion": "autoscaling/v1"}],
             "preferredVersion": {"groupVersion": "autoscaling/v2"}},
            {"name": "no.preference", "versions": [{"groupVersion": "no.preference/v1alpha1"}]}
        ]});
        assert_eq!(
            preferred_group_versions(&apis),
            vec!["apps/v1", "autoscaling/v2", "no.preference/v1alpha1"]
        );
    }

    #[test]
    fn only_listable_kinds_reach_the_sidebar() {
        let catalogue = catalogue(vec![vec![
            resource("", "v1", "Pod", "pods", &["list", "watch"]),
            resource("", "v1", "Pod", "pods/log", &["get"]),
            resource("", "v1", "Binding", "bindings", &["create"]),
        ]]);
        assert_eq!(catalogue.resources.len(), 1);
        assert!(catalogue.has(&ResourceKey::new("", "Pod")));
    }

    #[test]
    fn a_kind_served_twice_appears_once() {
        let catalogue = catalogue(vec![
            vec![resource(
                "apps",
                "v1",
                "Deployment",
                "deployments",
                &["list"],
            )],
            vec![resource(
                "apps",
                "v1beta1",
                "Deployment",
                "deployments",
                &["list"],
            )],
        ]);
        assert_eq!(catalogue.resources.len(), 1);
        assert_eq!(
            catalogue
                .get(&ResourceKey::new("apps", "Deployment"))
                .unwrap()
                .version,
            "v1"
        );
    }

    #[test]
    fn the_two_kubernetes_event_apis_are_one_sidebar_resource() {
        let catalogue = catalogue(vec![
            vec![resource(
                "events.k8s.io",
                "v1",
                "Event",
                "events",
                &["list", "watch"],
            )],
            vec![resource("", "v1", "Event", "events", &["list", "watch"])],
        ]);

        assert_eq!(
            catalogue
                .resources
                .iter()
                .filter(|resource| resource.kind == "Event")
                .count(),
            1
        );
        assert!(catalogue.has(&ResourceKey::new("", "Event")));
        assert!(!catalogue.has(&ResourceKey::new("events.k8s.io", "Event")));
    }

    #[test]
    fn the_sidebar_order_is_the_one_people_already_know() {
        let catalogue = catalogue(vec![vec![
            resource("apps", "v1", "Deployment", "deployments", &["list"]),
            resource("", "v1", "Pod", "pods", &["list"]),
            resource("", "v1", "Node", "nodes", &["list"]),
            resource("", "v1", "Service", "services", &["list"]),
            resource("argoproj.io", "v1alpha1", "Rollout", "rollouts", &["list"]),
        ]]);
        let kinds: Vec<&str> = catalogue
            .resources
            .iter()
            .map(|resource| resource.kind.as_str())
            .collect();
        assert_eq!(
            kinds,
            vec!["Node", "Pod", "Deployment", "Service", "Rollout"]
        );
    }

    #[test]
    fn a_custom_resource_needs_no_code_here() {
        let rollout = resource("argoproj.io", "v1alpha1", "Rollout", "rollouts", &["list"]);
        assert_eq!(group_of(&rollout), Group::Custom);
        let catalogue = catalogue(vec![vec![rollout]]);
        assert_eq!(catalogue.in_group(Group::Custom).len(), 1);
        assert!(catalogue.in_group(Group::Workloads).is_empty());
    }

    #[test]
    fn argo_cd_has_a_gitops_group_without_swallowing_argo_rollouts() {
        let application = resource(
            "argoproj.io",
            "v1alpha1",
            "Application",
            "applications",
            &["list"],
        );
        let rollout = resource("argoproj.io", "v1alpha1", "Rollout", "rollouts", &["list"]);
        assert_eq!(group_of(&application), Group::GitOps);
        assert_eq!(group_of(&rollout), Group::Custom);
    }

    #[test]
    fn kinds_of_one_custom_group_end_up_adjacent() {
        let catalogue = catalogue(vec![vec![
            resource("a.example", "v1", "Beta", "betas", &["list"]),
            resource("b.example", "v1", "Alpha", "alphas", &["list"]),
            resource("a.example", "v1", "Alpha", "alphas", &["list"]),
        ]]);
        let groups: Vec<&str> = catalogue
            .resources
            .iter()
            .map(|resource| resource.group.as_str())
            .collect();
        assert_eq!(groups, vec!["a.example", "a.example", "b.example"]);
    }

    #[test]
    fn every_built_in_group_has_something_in_the_order_list() {
        for group in Group::ALL {
            if *group == Group::Custom {
                continue;
            }
            assert!(
                ORDER.iter().any(|(_, _, named)| named == group),
                "{group:?} has no kinds"
            );
        }
    }
}
