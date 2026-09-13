//! The sidebar's tree, built from what the cluster serves.
//!
//! No row here is written in the source (`AGENTS.md` rule 8): the groups are
//! a *presentation* of the catalogue, and everything the groups do not name
//! falls into Custom Resources under its API group. A cluster running Argo
//! Rollouts gets a Rollouts row because the apiserver mentioned it, not
//! because anybody released a version of this app.

use crate::assets::icon;
use kirikumo_kube::{ApiResource, Catalogue, Group, ResourceKey, discovery};

/// One row: a kind the reader can list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavRow {
    /// Which kind, stable across a cluster upgrade that moves its version.
    pub key: ResourceKey,
    /// What the row says: the kind, pluralised.
    pub title: String,
}

/// A run of rows under an optional heading.
///
/// Only Custom Resources has headings — one per API group — because that is
/// the only group where the reader needs to know *whose* kind this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subsection {
    /// The API group, for Custom Resources.
    pub heading: Option<String>,
    /// The kinds under it.
    pub rows: Vec<NavRow>,
}

/// One of the sidebar's groups, with what the cluster serves in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Which group.
    pub group: Group,
    /// Its rows.
    pub subsections: Vec<Subsection>,
}

impl Section {
    /// How many kinds are in this group.
    pub fn len(&self) -> usize {
        self.subsections
            .iter()
            .map(|subsection| subsection.rows.len())
            .sum()
    }

    /// Whether the group has nothing in it, which is when it is not drawn.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The sidebar's tree.
///
/// Groups with nothing in them are dropped rather than drawn empty: a cluster
/// where the reader may not list roles should not have an Access Control
/// heading with a void under it.
pub fn sections(catalogue: &Catalogue) -> Vec<Section> {
    Group::ALL
        .iter()
        .filter_map(|group| {
            let resources = catalogue.in_group(*group);
            if resources.is_empty() {
                return None;
            }
            let subsections = match group {
                Group::Custom => by_api_group(&resources),
                _ => vec![Subsection {
                    heading: None,
                    rows: resources.iter().map(|resource| row(resource)).collect(),
                }],
            };
            Some(Section {
                group: *group,
                subsections,
            })
        })
        .collect()
}

/// Custom resources, gathered under their API group.
///
/// The catalogue already sorts them so that one group's kinds are adjacent
/// (`discovery::catalogue`), so this is a fold rather than a grouping pass.
fn by_api_group(resources: &[&ApiResource]) -> Vec<Subsection> {
    let mut subsections: Vec<Subsection> = Vec::new();
    for resource in resources {
        let heading = Some(resource.group.clone()).filter(|group| !group.is_empty());
        match subsections.last_mut() {
            Some(last) if last.heading == heading => last.rows.push(row(resource)),
            _ => subsections.push(Subsection {
                heading,
                rows: vec![row(resource)],
            }),
        }
    }
    subsections
}

fn row(resource: &ApiResource) -> NavRow {
    NavRow {
        key: resource.key(),
        title: title(resource),
    }
}

/// What a row says.
///
/// The kind, pluralised in English, rather than the apiserver's plural path
/// segment: that segment is lowercase (`networkpolicies`), and a sidebar of
/// lowercase words is a sidebar nobody scans. A kind whose plural *is* its
/// own name — `Endpoints` — is left alone, which is the case the naive rule
/// gets wrong.
pub fn title(resource: &ApiResource) -> String {
    let kind = &resource.kind;
    if resource.name.eq_ignore_ascii_case(kind) {
        return kind.clone();
    }
    plural(kind)
}

/// English pluralisation, for the handful of shapes Kubernetes kind names
/// actually take.
fn plural(kind: &str) -> String {
    let lower = kind.to_lowercase();
    if lower.ends_with('s')
        || lower.ends_with('x')
        || lower.ends_with('z')
        || lower.ends_with("ch")
        || lower.ends_with("sh")
    {
        return format!("{kind}es");
    }
    // `Policy` → `Policies`, but `Gateway` → `Gateways`.
    if let Some(stem) = kind.strip_suffix('y')
        && !lower.ends_with("ay")
        && !lower.ends_with("ey")
        && !lower.ends_with("oy")
        && !lower.ends_with("uy")
    {
        return format!("{stem}ies");
    }
    format!("{kind}s")
}

/// The icon a group's heading carries.
pub fn group_icon(group: Group) -> &'static str {
    match group {
        Group::Cluster => icon::GLOBE,
        Group::Workloads => icon::LAYOUT,
        Group::Config => icon::SETTINGS,
        Group::Network => icon::NETWORK,
        Group::Storage => icon::DISK,
        Group::AccessControl => icon::USER,
        Group::GitOps => icon::FRAME,
        Group::Custom => icon::FRAME,
    }
}

/// A group's name in the settings file, for remembering which are folded.
///
/// Spelled out rather than derived from the enum's `Debug`, so renaming a
/// variant cannot silently unfold everybody's sidebar.
pub fn group_id(group: Group) -> &'static str {
    match group {
        Group::Cluster => "cluster",
        Group::Workloads => "workloads",
        Group::Config => "config",
        Group::Network => "network",
        Group::Storage => "storage",
        Group::AccessControl => "access-control",
        Group::GitOps => "gitops",
        Group::Custom => "custom",
    }
}

/// Which group a kind is in, for a sidebar that has to scroll to a selection.
pub fn group_of(resource: &ApiResource) -> Group {
    discovery::group_of(resource)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(group: &str, kind: &str, name: &str) -> ApiResource {
        ApiResource {
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            name: name.into(),
            singular: String::new(),
            namespaced: true,
            verbs: vec!["list".into()],
            short_names: Vec::new(),
            categories: Vec::new(),
        }
    }

    fn catalogue(resources: Vec<ApiResource>) -> Catalogue {
        discovery::catalogue(vec![resources])
    }

    #[test]
    fn a_row_says_the_kind_pluralised_and_not_the_apiservers_lowercase_path() {
        assert_eq!(title(&resource("", "Pod", "pods")), "Pods");
        assert_eq!(
            title(&resource(
                "networking.k8s.io",
                "NetworkPolicy",
                "networkpolicies"
            )),
            "NetworkPolicies"
        );
        assert_eq!(
            title(&resource("networking.k8s.io", "Ingress", "ingresses")),
            "Ingresses"
        );
        assert_eq!(
            title(&resource(
                "",
                "ReplicationController",
                "replicationcontrollers"
            )),
            "ReplicationControllers"
        );
    }

    #[test]
    fn a_kind_whose_plural_is_its_own_name_is_left_alone() {
        // The case the naive rule gets wrong: `Endpointses`.
        assert_eq!(title(&resource("", "Endpoints", "endpoints")), "Endpoints");
        assert_eq!(
            title(&resource("", "ComponentStatus", "componentstatuses")),
            "ComponentStatuses"
        );
    }

    #[test]
    fn a_y_after_a_vowel_is_not_an_ies() {
        assert_eq!(plural("Gateway"), "Gateways");
        assert_eq!(plural("Policy"), "Policies");
        assert_eq!(plural("Proxy"), "Proxies");
    }

    #[test]
    fn the_tree_is_the_catalogue_in_the_sidebars_order() {
        let sections = sections(&catalogue(vec![
            resource("apps", "Deployment", "deployments"),
            resource("", "Pod", "pods"),
            resource("", "Node", "nodes"),
            resource("", "Service", "services"),
        ]));
        let groups: Vec<Group> = sections.iter().map(|section| section.group).collect();
        assert_eq!(
            groups,
            vec![Group::Cluster, Group::Workloads, Group::Network]
        );
        assert_eq!(sections[1].len(), 2);
    }

    #[test]
    fn a_group_the_cluster_serves_nothing_in_is_not_drawn_at_all() {
        let sections = sections(&catalogue(vec![resource("", "Pod", "pods")]));
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].group, Group::Workloads);
    }

    #[test]
    fn custom_resources_are_gathered_under_their_api_group() {
        let sections = sections(&catalogue(vec![
            resource("argoproj.io", "Rollout", "rollouts"),
            resource("argoproj.io", "AnalysisRun", "analysisruns"),
            resource("cert-manager.io", "Certificate", "certificates"),
            resource("", "Pod", "pods"),
        ]));
        let custom = sections
            .iter()
            .find(|section| section.group == Group::Custom)
            .unwrap();
        let headings: Vec<Option<&str>> = custom
            .subsections
            .iter()
            .map(|subsection| subsection.heading.as_deref())
            .collect();
        assert_eq!(headings, vec![Some("argoproj.io"), Some("cert-manager.io")]);
        assert_eq!(custom.subsections[0].rows.len(), 2);
        assert_eq!(custom.subsections[0].rows[0].title, "AnalysisRuns");
    }

    #[test]
    fn argo_cd_is_its_own_optional_group_and_rollouts_stay_custom() {
        let sections = sections(&catalogue(vec![
            resource("argoproj.io", "Application", "applications"),
            resource("argoproj.io", "ApplicationSet", "applicationsets"),
            resource("argoproj.io", "Rollout", "rollouts"),
        ]));
        let gitops = sections
            .iter()
            .find(|section| section.group == Group::GitOps)
            .unwrap();
        assert_eq!(gitops.len(), 2);
        let custom = sections
            .iter()
            .find(|section| section.group == Group::Custom)
            .unwrap();
        assert_eq!(custom.len(), 1);
        assert_eq!(custom.subsections[0].rows[0].title, "Rollouts");
    }

    #[test]
    fn the_built_in_groups_carry_no_headings_because_everyone_knows_whose_pods_they_are() {
        let sections = sections(&catalogue(vec![resource("", "Pod", "pods")]));
        assert_eq!(sections[0].subsections.len(), 1);
        assert!(sections[0].subsections[0].heading.is_none());
    }

    #[test]
    fn every_group_has_an_icon_and_a_name_that_can_be_written_to_a_file() {
        let mut ids: Vec<&str> = Group::ALL.iter().map(|group| group_id(*group)).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Group::ALL.len());
        for group in Group::ALL {
            assert!(group_icon(*group).ends_with(".svg"));
            assert!(!group.label_key().is_empty());
        }
    }

    #[test]
    fn a_row_is_keyed_by_kind_and_group_so_a_version_bump_keeps_the_selection() {
        let sections = sections(&catalogue(vec![resource(
            "apps",
            "Deployment",
            "deployments",
        )]));
        assert_eq!(
            sections[0].subsections[0].rows[0].key,
            ResourceKey::new("apps", "Deployment")
        );
    }
}
