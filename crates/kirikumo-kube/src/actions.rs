//! What can be done to an object, and what each thing becomes on the wire.
//!
//! The viewer's few writes (`docs/roadmap.md` M4): sync, delete, scale, restart,
//! cordon and uncordon, and applying an edited manifest. Each is decided here
//! — which kinds offer it, which RBAC verb it needs, what patch it is — so
//! the rules can be tested without a window or a cluster, and so the view
//! that draws the buttons holds no opinion about Kubernetes.
//!
//! Every one of these is reached from the object it acts on and confirmed
//! there, never from a key chord and never from the palette (`AGENTS.md`
//! rule 9). This module does not know that; the views enforce it.

use crate::error::{Error, Result};
use crate::model::{ApiResource, Object, Patch};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

/// A write the viewer offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Ask an Argo CD Application to reconcile its configured revision.
    Sync,
    /// Change how many replicas a controller wants.
    Scale,
    /// Roll every pod of a controller, the way `kubectl rollout restart` does.
    Restart,
    /// Stop scheduling onto a node.
    Cordon,
    /// Schedule onto it again.
    Uncordon,
    /// Cordon a node and move everything off it that can move
    /// ([`crate::drain`]).
    Drain,
    /// Replace the object with an edited manifest.
    Apply,
    /// Remove the object.
    Delete,
}

impl Action {
    /// Every action, in the order a strip of buttons lists them: the mild
    /// ones first, and delete last where a hand does not fall on it.
    pub const ALL: &'static [Action] = &[
        Action::Sync,
        Action::Scale,
        Action::Restart,
        Action::Cordon,
        Action::Uncordon,
        Action::Drain,
        Action::Apply,
        Action::Delete,
    ];

    /// The RBAC verb the action needs, for `SelfSubjectAccessReview`.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            // A drain is a cordon and then evictions; the cordon is what
            // can be asked about up front. Whether each eviction is allowed
            // is answered by the apiserver, per pod, in the report.
            Self::Sync
            | Self::Scale
            | Self::Restart
            | Self::Cordon
            | Self::Uncordon
            | Self::Drain => "patch",
            Self::Apply => "update",
        }
    }

    /// The locale key for the action's name.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Sync => "action.sync",
            Self::Scale => "action.scale",
            Self::Restart => "action.restart",
            Self::Cordon => "action.cordon",
            Self::Uncordon => "action.uncordon",
            Self::Drain => "action.drain",
            Self::Apply => "action.apply",
            Self::Delete => "action.delete",
        }
    }

    /// Whether the action removes something, which is drawn in the error
    /// colour so that a button that cannot be undone looks like it. A drain
    /// counts: it evicts every pod on the node.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Delete | Self::Drain)
    }
}

/// Which actions an object offers.
///
/// Decided from what the apiserver said the kind supports — a kind without
/// `delete` in its verbs gets no delete button, however much a person would
/// like one — and from what the kind *is*: only a controller scales, only a
/// node cordons. Whether *this reader* may do it is a separate question, asked
/// of the apiserver per verb, and answered by greying the button out.
pub fn available(resource: &ApiResource, object: &Object) -> Vec<Action> {
    let kind = resource.kind.as_str();
    let scalable = matches!(kind, "Deployment" | "StatefulSet" | "ReplicaSet");
    let restartable = matches!(kind, "Deployment" | "StatefulSet" | "DaemonSet");
    let mut actions = Vec::new();
    if resource.supports("patch") {
        let argo_application = resource.group == "argoproj.io" && kind == "Application";
        let operation = object.str_at("status.operationState.phase");
        let idle = !matches!(operation, "Running" | "Terminating")
            && object.at("operation").is_none_or(Value::is_null);
        if argo_application && idle {
            actions.push(Action::Sync);
        }
        if scalable {
            actions.push(Action::Scale);
        }
        if restartable {
            actions.push(Action::Restart);
        }
        if kind == "Node" {
            actions.push(match object.bool_at("spec.unschedulable") {
                true => Action::Uncordon,
                false => Action::Cordon,
            });
            actions.push(Action::Drain);
        }
    }
    if resource.supports("update") {
        actions.push(Action::Apply);
    }
    if resource.supports("delete") {
        actions.push(Action::Delete);
    }
    actions
}

/// How many replicas a controller wants right now.
///
/// `spec.replicas` defaults to one when absent, as the apiserver defaults it.
pub fn current_replicas(object: &Object) -> i64 {
    object
        .at("spec.replicas")
        .and_then(Value::as_i64)
        .unwrap_or(1)
}

/// The patch that scales a controller.
pub fn scale(replicas: u32) -> Patch {
    Patch::Merge(json!({"spec": {"replicas": replicas}}))
}

/// The patch that restarts a controller's pods.
///
/// What `kubectl rollout restart` does: stamp the pod template with the time,
/// which changes the template, which makes the controller roll. Strategic
/// rather than merge, because the template's containers are a list and only
/// a strategic merge leaves a list alone when the patch does not mention it.
pub fn restart(now: DateTime<Utc>) -> Patch {
    Patch::Strategic(json!({
        "spec": {"template": {"metadata": {"annotations": {
            "kubectl.kubernetes.io/restartedAt": now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        }}}}
    }))
}

/// Ask Argo CD to sync the whole Application at its configured revision.
///
/// Revision, resource filters and prune are deliberately absent: this is the
/// least surprising equivalent of `argocd app sync NAME`, and cannot inherit
/// a selective resource list from Kirikumo because Kirikumo never creates
/// one. Argo's controller consumes and clears the top-level operation.
pub fn sync() -> Patch {
    Patch::Merge(json!({
        "operation": {
            "initiatedBy": {"username": "kirikumo"},
            "sync": {}
        }
    }))
}

/// The patch that cordons a node, or uncordons it.
pub fn schedulable(unschedulable: bool) -> Patch {
    Patch::Merge(json!({"spec": {"unschedulable": unschedulable}}))
}

/// The replacement an edited manifest becomes.
///
/// Parsed rather than sent as text so that a manifest that is not a manifest
/// — not YAML, not an object, or missing its name — is refused here, with a
/// reason, rather than by the apiserver with a 400 and a stack of quotes.
pub fn apply(manifest: &str) -> Result<Patch> {
    let value: Value = serde_norway::from_str(manifest)
        .map_err(|error| Error::Malformed(format!("not YAML: {error}")))?;
    if !value.is_object() {
        return Err(Error::Malformed("a manifest is a mapping".into()));
    }
    // The same check a list's items get: an object with no name is not one
    // the apiserver can put anywhere.
    Object::new(value.clone())?;
    Ok(Patch::Replace(value))
}

/// Apply a JSON merge patch (RFC 7386) to a value, in place.
///
/// What the *scripted* cluster does with a patch, so that a demo delete or
/// scale is real. A live apiserver merges for itself; this exists so the
/// fake behaves the way the real thing does for the patches this app sends.
/// A strategic merge patch is applied with the same rule, which is right for
/// every patch this app makes — none of them touch a list — and wrong in
/// general, which the scripted cluster says in its rustdoc.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(changes) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    let Value::Object(fields) = target else {
        unreachable!("made an object just above");
    };
    for (key, change) in changes {
        match change {
            Value::Null => {
                fields.remove(key);
            }
            Value::Object(_) => {
                let slot = fields.entry(key.clone()).or_insert(Value::Null);
                merge_patch(slot, change);
            }
            other => {
                fields.insert(key.clone(), other.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(kind: &str, verbs: &[&str]) -> ApiResource {
        ApiResource {
            group: String::new(),
            version: "v1".into(),
            kind: kind.into(),
            name: kind.to_lowercase(),
            singular: kind.to_lowercase(),
            namespaced: true,
            verbs: verbs.iter().map(|verb| verb.to_string()).collect(),
            short_names: Vec::new(),
            categories: Vec::new(),
        }
    }

    fn object(value: Value) -> Object {
        let mut value = value;
        if value.get("metadata").is_none() {
            value["metadata"] = json!({"name": "x"});
        }
        Object::new(value).unwrap()
    }

    const FULL: &[&str] = &["get", "list", "watch", "patch", "update", "delete"];

    #[test]
    fn a_deployment_scales_restarts_applies_and_deletes_in_that_order() {
        let actions = available(&resource("Deployment", FULL), &object(json!({})));
        assert_eq!(
            actions,
            vec![
                Action::Scale,
                Action::Restart,
                Action::Apply,
                Action::Delete
            ]
        );
    }

    #[test]
    fn a_pod_only_applies_and_deletes() {
        let actions = available(&resource("Pod", FULL), &object(json!({})));
        assert_eq!(actions, vec![Action::Apply, Action::Delete]);
    }

    #[test]
    fn a_daemonset_restarts_but_does_not_scale() {
        let actions = available(&resource("DaemonSet", FULL), &object(json!({})));
        assert!(actions.contains(&Action::Restart));
        assert!(!actions.contains(&Action::Scale));
    }

    #[test]
    fn only_an_idle_argo_application_offers_sync() {
        let mut application = resource("Application", FULL);
        application.group = "argoproj.io".into();
        let idle = available(&application, &object(json!({"status": {}})));
        assert!(idle.contains(&Action::Sync));

        let running = available(
            &application,
            &object(json!({"status": {"operationState": {"phase": "Running"}}})),
        );
        assert!(!running.contains(&Action::Sync));

        let unrelated = available(
            &resource("Application", FULL),
            &object(json!({"status": {}})),
        );
        assert!(!unrelated.contains(&Action::Sync));
    }

    #[test]
    fn a_node_offers_cordon_or_uncordon_depending_on_where_it_is() {
        let node = resource("Node", FULL);
        assert!(available(&node, &object(json!({"spec": {}}))).contains(&Action::Cordon));
        let cordoned = object(json!({"spec": {"unschedulable": true}}));
        let actions = available(&node, &cordoned);
        assert!(actions.contains(&Action::Uncordon));
        assert!(!actions.contains(&Action::Cordon));
    }

    #[test]
    fn a_kind_the_apiserver_will_not_let_anyone_delete_gets_no_delete_button() {
        let readonly = resource("Deployment", &["get", "list", "watch"]);
        assert!(available(&readonly, &object(json!({}))).is_empty());
        let no_delete = resource("Deployment", &["get", "list", "patch", "update"]);
        let actions = available(&no_delete, &object(json!({})));
        assert!(!actions.contains(&Action::Delete));
        assert!(actions.contains(&Action::Scale));
    }

    #[test]
    fn delete_is_always_last_and_only_it_and_drain_are_destructive() {
        for action in Action::ALL {
            assert_eq!(
                action.is_destructive(),
                matches!(action, Action::Delete | Action::Drain),
                "{action:?}"
            );
        }
        assert_eq!(Action::ALL.last(), Some(&Action::Delete));
    }

    #[test]
    fn a_node_can_be_drained_whichever_way_it_is_cordoned() {
        let node = resource("Node", FULL);
        for spec in [
            json!({"spec": {}}),
            json!({"spec": {"unschedulable": true}}),
        ] {
            assert!(available(&node, &object(spec)).contains(&Action::Drain));
        }
        // But not a kind that is not a node.
        assert!(!available(&resource("Pod", FULL), &object(json!({}))).contains(&Action::Drain));
    }

    #[test]
    fn each_action_names_the_verb_rbac_will_be_asked_about() {
        assert_eq!(Action::Delete.verb(), "delete");
        assert_eq!(Action::Sync.verb(), "patch");
        assert_eq!(Action::Scale.verb(), "patch");
        assert_eq!(Action::Restart.verb(), "patch");
        assert_eq!(Action::Cordon.verb(), "patch");
        assert_eq!(Action::Drain.verb(), "patch");
        assert_eq!(Action::Apply.verb(), "update");
    }

    #[test]
    fn scaling_is_a_merge_of_one_field() {
        assert_eq!(scale(3), Patch::Merge(json!({"spec": {"replicas": 3}})));
        assert_eq!(
            current_replicas(&object(json!({"spec": {"replicas": 5}}))),
            5
        );
        assert_eq!(current_replicas(&object(json!({"spec": {}}))), 1);
    }

    #[test]
    fn restarting_stamps_the_template_the_way_kubectl_does() {
        let now = DateTime::parse_from_rfc3339("2026-09-12T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let Patch::Strategic(body) = restart(now) else {
            panic!("a restart must be a strategic merge, or the containers list is lost");
        };
        assert_eq!(
            body.pointer("/spec/template/metadata/annotations/kubectl.kubernetes.io~1restartedAt"),
            Some(&json!("2026-09-12T10:00:00Z"))
        );
    }

    #[test]
    fn syncing_asks_argo_for_a_full_non_pruning_sync_at_the_configured_revision() {
        assert_eq!(
            sync(),
            Patch::Merge(json!({
                "operation": {
                    "initiatedBy": {"username": "kirikumo"},
                    "sync": {}
                }
            }))
        );
    }

    #[test]
    fn cordoning_sets_the_flag_and_uncordoning_clears_it() {
        assert_eq!(
            schedulable(true),
            Patch::Merge(json!({"spec": {"unschedulable": true}}))
        );
        assert_eq!(
            schedulable(false),
            Patch::Merge(json!({"spec": {"unschedulable": false}}))
        );
    }

    #[test]
    fn an_edited_manifest_is_parsed_and_becomes_a_replacement() {
        let patch = apply("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\ndata:\n  a: b\n")
            .unwrap();
        let Patch::Replace(value) = patch else {
            panic!("an apply is a PUT of the whole object");
        };
        assert_eq!(value["data"]["a"], json!("b"));
    }

    #[test]
    fn a_manifest_that_is_not_one_is_refused_here_with_a_reason() {
        assert!(matches!(apply("{ not yaml"), Err(Error::Malformed(_))));
        assert!(matches!(apply("- a\n- list\n"), Err(Error::Malformed(_))));
        assert!(matches!(
            apply("apiVersion: v1\nkind: ConfigMap\ndata: {}\n"),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn a_merge_patch_changes_what_it_names_and_leaves_the_rest() {
        let mut target = json!({"spec": {"replicas": 1, "selector": {"app": "x"}}, "status": {}});
        merge_patch(&mut target, &json!({"spec": {"replicas": 3}}));
        assert_eq!(target["spec"]["replicas"], json!(3));
        assert_eq!(target["spec"]["selector"]["app"], json!("x"));
        assert!(target.get("status").is_some());
    }

    #[test]
    fn a_null_in_a_merge_patch_removes_the_field() {
        let mut target = json!({"spec": {"unschedulable": true, "taints": []}});
        merge_patch(&mut target, &json!({"spec": {"unschedulable": null}}));
        assert!(target["spec"].get("unschedulable").is_none());
        assert!(target["spec"].get("taints").is_some());
    }

    #[test]
    fn a_merge_patch_creates_the_path_it_needs() {
        let mut target = json!({"spec": {}});
        merge_patch(
            &mut target,
            &json!({"spec": {"template": {"metadata": {"annotations": {"k": "v"}}}}}),
        );
        assert_eq!(
            target.pointer("/spec/template/metadata/annotations/k"),
            Some(&json!("v"))
        );
    }

    #[test]
    fn a_merge_patch_that_is_not_an_object_replaces_outright() {
        let mut target = json!({"a": 1});
        merge_patch(&mut target, &json!(7));
        assert_eq!(target, json!(7));
    }
}
