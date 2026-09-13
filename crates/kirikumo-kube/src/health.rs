//! The rule that turns an object into the mark at the head of its row.
//!
//! One mark and one word per object (`docs/ui.md` §1.6). The mark is the
//! glance — is anything wrong on this page — and the word is the answer, so
//! the two are produced together and neither is optional.
//!
//! Written per kind against the JSON, never against a generated type. A kind
//! with no rule of its own still gets one: the fallback reads `Ready` or
//! `Available` out of `status.conditions`, which is the convention nearly
//! every controller follows, so a custom resource arrives with a working mark
//! and nobody wrote it (`AGENTS.md` rule 8).

use crate::model::{Object, ResourceKey};
use serde_json::Value;

/// How worried to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Fine.
    Ok,
    /// On its way somewhere: pending, creating, rolling out, terminating.
    Working,
    /// Working, but not as intended: fewer ready than wanted, a node under
    /// pressure, a claim nothing has bound.
    Attention,
    /// Broken.
    Error,
    /// Nothing to read. A kind with no status, or one we have no rule for.
    Unknown,
}

impl Level {
    /// The theme token this level paints in (`docs/ui.md` §2).
    ///
    /// Named here rather than in the views so the mapping is in one place and
    /// can be asserted; the crate has no `gpui` dependency and returns the
    /// token's name, not a colour.
    pub fn token(self) -> &'static str {
        match self {
            Self::Ok => "status.done",
            Self::Working => "status.working",
            Self::Attention => "status.attention",
            Self::Error => "status.error",
            Self::Unknown => "text.muted",
        }
    }

    /// Whether this level is worth pulling a reader's eye to, which is what
    /// a cluster-wide summary counts.
    pub fn is_bad(self) -> bool {
        matches!(self, Self::Attention | Self::Error)
    }
}

/// A mark and the word beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// The mark.
    pub level: Level,
    /// What to write next to it: `Running`, `CrashLoopBackOff`, `3/5`.
    pub word: String,
}

impl Health {
    /// A health from its parts.
    pub fn new(level: Level, word: impl Into<String>) -> Self {
        Self {
            level,
            word: word.into(),
        }
    }

    /// Nothing to say.
    pub fn unknown() -> Self {
        Self::new(Level::Unknown, "")
    }
}

/// The container-state reasons that mean a pod is broken rather than busy.
///
/// The distinction matters more than it looks: `ContainerCreating` and
/// `CrashLoopBackOff` are both `waiting`, and a viewer that paints them the
/// same is a viewer nobody trusts.
const FATAL_WAITING: &[&str] = &[
    "CrashLoopBackOff",
    "ImagePullBackOff",
    "ErrImagePull",
    "CreateContainerConfigError",
    "CreateContainerError",
    "InvalidImageName",
    "RunContainerError",
    "CniNetworkError",
];

/// The health of an object of a kind.
///
/// `kind` is passed rather than read from the object because a list's items
/// routinely omit it — the apiserver puts it on the list, not on each entry —
/// and the caller always knows which table it is filling.
pub fn of(kind: &str, object: &Object) -> Health {
    of_resource(&ResourceKey::new("", kind), object)
}

/// The health of an object when its full catalogue identity is known.
///
/// This is the form tables and details use. The API group prevents a custom
/// kind with an ordinary name such as `Application` from receiving another
/// controller's semantics.
pub fn of_resource(resource: &ResourceKey, object: &Object) -> Health {
    // A deletion in flight outranks every kind's own rule: an object being
    // torn down is not unhealthy, and reporting its half-gone state as an
    // error is how a viewer cries wolf during a rollout.
    if object.meta.is_terminating() {
        return Health::new(Level::Working, "Terminating");
    }
    if resource.group == "argoproj.io" && resource.kind == "Application" {
        return application(object);
    }
    if resource.group == "argoproj.io" && resource.kind == "ApplicationSet" {
        return application_set(object);
    }
    match resource.kind.as_str() {
        "Pod" => pod(object),
        "Node" => node(object),
        "Deployment" | "StatefulSet" | "ReplicaSet" | "ReplicationController" => {
            replicas(object, "spec.replicas", "status.readyReplicas")
        }
        "DaemonSet" => replicas(
            object,
            "status.desiredNumberScheduled",
            "status.numberReady",
        ),
        "Job" => job(object),
        "CronJob" => cronjob(object),
        "PersistentVolumeClaim" => phase(
            object,
            &[
                ("Bound", Level::Ok),
                ("Pending", Level::Working),
                ("Lost", Level::Error),
            ],
        ),
        "PersistentVolume" => phase(
            object,
            &[
                ("Bound", Level::Ok),
                ("Available", Level::Ok),
                ("Pending", Level::Working),
                ("Released", Level::Attention),
                ("Failed", Level::Error),
            ],
        ),
        "Namespace" => phase(
            object,
            &[("Active", Level::Ok), ("Terminating", Level::Working)],
        ),
        "Service" => service(object),
        "Event" => event(object),
        _ => fallback(object),
    }
}

/// How many of a pod's containers are ready, and how many there are.
///
/// The `READY` column, and the thing that decides whether a `Running` pod is
/// green or amber. Init containers are deliberately not counted: `kubectl`
/// does not count them either, and a pod still running its init containers is
/// `Pending`, not `1/2`.
pub fn ready_containers(object: &Object) -> (usize, usize) {
    let statuses = object.array_at("status.containerStatuses");
    let total = match statuses.is_empty() {
        // Before the kubelet reports, the spec is the only place the count
        // is, and `0/0` for a pod with two containers reads as an empty pod.
        true => object.array_at("spec.containers").len(),
        false => statuses.len(),
    };
    let ready = statuses
        .iter()
        .filter(|status| status.get("ready").and_then(Value::as_bool) == Some(true))
        .count();
    (ready, total)
}

/// How many times a pod's containers have been restarted, in total.
pub fn restarts(object: &Object) -> i64 {
    object
        .array_at("status.containerStatuses")
        .iter()
        .filter_map(|status| status.get("restartCount").and_then(Value::as_i64))
        .sum()
}

fn pod(object: &Object) -> Health {
    let phase = object.str_at("status.phase");
    // A pod that finished on purpose. `kubectl` writes `Completed`, and the
    // phase alone says `Succeeded`, which reads like an error code.
    if phase == "Succeeded" {
        return Health::new(Level::Ok, "Completed");
    }
    if phase == "Failed" {
        let reason = object.str_at("status.reason");
        return Health::new(
            Level::Error,
            if reason.is_empty() { "Failed" } else { reason },
        );
    }
    // A container's own reason beats the phase, because a pod in
    // `CrashLoopBackOff` still has phase `Running`.
    if let Some(reason) = waiting_reason(object) {
        let level = match FATAL_WAITING.contains(&reason.as_str()) {
            true => Level::Error,
            false => Level::Working,
        };
        return Health::new(level, reason);
    }
    if let Some(reason) = terminated_failure(object) {
        return Health::new(Level::Error, reason);
    }
    match phase {
        "Pending" => Health::new(Level::Working, "Pending"),
        "Running" => {
            let (ready, total) = ready_containers(object);
            match ready == total && total > 0 {
                true => Health::new(Level::Ok, "Running"),
                // Running but short of its containers: not broken, not right.
                false => Health::new(Level::Attention, "Running"),
            }
        }
        // Phase `Unknown` is what the apiserver says when it has lost the
        // node the pod is on, which is a failure and not an absence.
        "Unknown" => Health::new(Level::Error, "Unknown"),
        "" => Health::unknown(),
        other => Health::new(Level::Unknown, other),
    }
}

/// The first waiting reason among a pod's containers, init containers first.
///
/// Init containers come first because they run first: a pod whose init
/// container cannot pull its image is stuck there, and the app container's
/// `PodInitializing` is the symptom, not the cause.
fn waiting_reason(object: &Object) -> Option<String> {
    for path in ["status.initContainerStatuses", "status.containerStatuses"] {
        // A fatal reason anywhere beats a benign one earlier in the list.
        let mut benign = None;
        for status in object.array_at(path) {
            let Some(reason) = status
                .pointer("/state/waiting/reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
            else {
                continue;
            };
            if FATAL_WAITING.contains(&reason) {
                return Some(reason.to_string());
            }
            benign.get_or_insert_with(|| reason.to_string());
        }
        if let Some(reason) = benign {
            return Some(reason);
        }
    }
    None
}

/// A container that stopped with a non-zero exit, when nothing is waiting.
fn terminated_failure(object: &Object) -> Option<String> {
    object
        .array_at("status.containerStatuses")
        .iter()
        .find_map(|status| {
            let terminated = status.pointer("/state/terminated")?;
            let code = terminated.get("exitCode").and_then(Value::as_i64)?;
            if code == 0 {
                return None;
            }
            let reason = terminated
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
                .unwrap_or("Error");
            Some(reason.to_string())
        })
}

fn node(object: &Object) -> Health {
    let unschedulable = object.bool_at("spec.unschedulable");
    if object.condition_is("Ready", "True") {
        return match unschedulable {
            // Cordoned. Healthy, deliberately taken out of service, and the
            // one state a viewer must never paint green without saying so.
            true => Health::new(Level::Attention, "Ready,SchedulingDisabled"),
            false => {
                let pressure = ["MemoryPressure", "DiskPressure", "PIDPressure"]
                    .into_iter()
                    .find(|condition| object.condition_is(condition, "True"));
                match pressure {
                    Some(condition) => Health::new(Level::Attention, condition),
                    None => Health::new(Level::Ok, "Ready"),
                }
            }
        };
    }
    if object.condition_is("Ready", "False") {
        return Health::new(Level::Error, "NotReady");
    }
    if object.condition("Ready").is_some() {
        // `Unknown` means the kubelet has stopped reporting, which is the
        // worst of the three and not an absence of information.
        return Health::new(Level::Error, "Unknown");
    }
    Health::unknown()
}

/// The shape shared by every controller that keeps a number of things alive.
fn replicas(object: &Object, desired_path: &str, ready_path: &str) -> Health {
    // A Deployment's `spec.replicas` defaults to 1 when it is absent; a
    // DaemonSet's desired count is always reported, so the default never
    // applies there.
    let desired = match object.at(desired_path) {
        Some(value) => value.as_i64().unwrap_or(0),
        None if desired_path.starts_with("spec.") => 1,
        None => 0,
    };
    let ready = object.int_at(ready_path);
    let word = format!("{ready}/{desired}");
    if desired == 0 {
        // Deliberately scaled to nothing. Not an error, not health either.
        return Health::new(Level::Unknown, word);
    }
    if ready >= desired {
        return Health::new(Level::Ok, word);
    }
    match ready {
        0 => Health::new(Level::Error, word),
        _ => Health::new(Level::Attention, word),
    }
}

fn job(object: &Object) -> Health {
    if object.condition_is("Failed", "True") {
        return Health::new(Level::Error, "Failed");
    }
    let completions = object
        .at("spec.completions")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let succeeded = object.int_at("status.succeeded");
    if object.condition_is("Complete", "True") || succeeded >= completions {
        return Health::new(Level::Ok, format!("{succeeded}/{completions}"));
    }
    match object.int_at("status.active") {
        0 => Health::new(Level::Working, format!("{succeeded}/{completions}")),
        active => Health::new(Level::Working, format!("{active} running")),
    }
}

fn cronjob(object: &Object) -> Health {
    match object.bool_at("spec.suspend") {
        // Suspended is a decision, not a fault — but a suspended CronJob is
        // the single most common reason a person asks why something did not
        // run, so it is never silent.
        true => Health::new(Level::Attention, "Suspended"),
        false => Health::new(Level::Ok, "Active"),
    }
}

fn service(object: &Object) -> Health {
    let kind = object.str_at("spec.type");
    if kind == "LoadBalancer" && object.array_at("status.loadBalancer.ingress").is_empty() {
        return Health::new(Level::Working, "Pending");
    }
    match kind.is_empty() {
        true => Health::new(Level::Ok, "ClusterIP"),
        false => Health::new(Level::Ok, kind),
    }
}

fn event(object: &Object) -> Health {
    match object.str_at("type") {
        "Warning" => Health::new(Level::Attention, "Warning"),
        "" => Health::new(Level::Ok, "Normal"),
        other => Health::new(Level::Ok, other),
    }
}

/// An Argo CD Application's reconciliation state.
///
/// Argo CD does not expose a conventional `Ready` condition. Its own health,
/// sync and operation words are the authoritative state, so the generic CRD
/// fallback would otherwise leave every Application grey.
fn application(object: &Object) -> Health {
    let operation = object.str_at("status.operationState.phase");
    match operation {
        "Running" | "Terminating" => return Health::new(Level::Working, operation),
        "Error" | "Failed" => return Health::new(Level::Error, operation),
        _ => {}
    }

    let health = object.str_at("status.health.status");
    match health {
        "Degraded" | "Missing" => return Health::new(Level::Error, health),
        "Progressing" => return Health::new(Level::Working, health),
        "Suspended" => return Health::new(Level::Attention, health),
        _ => {}
    }

    let sync = object.str_at("status.sync.status");
    match (sync, health) {
        ("Synced", "Healthy") => Health::new(Level::Ok, "Synced"),
        ("OutOfSync", _) => Health::new(Level::Attention, "OutOfSync"),
        (_, "Healthy") => Health::new(Level::Ok, "Healthy"),
        (_, "Unknown") => Health::new(Level::Unknown, "Unknown"),
        ("", "") => Health::unknown(),
        (_, other) if !other.is_empty() => Health::new(Level::Unknown, other),
        (other, _) => Health::new(Level::Unknown, other),
    }
}

/// An Argo CD ApplicationSet's generated-Application health.
///
/// Newer controllers persist the calculated health directly. Older ones
/// expose only conditions, for which this follows Argo's own priority:
/// errors, stale resources, a progressing rollout, then up-to-date resources.
fn application_set(object: &Object) -> Health {
    let health = object.str_at("status.health.status");
    if !health.is_empty() {
        return match health {
            "Healthy" => Health::new(Level::Ok, health),
            "Degraded" | "Missing" => Health::new(Level::Error, health),
            "Progressing" => Health::new(Level::Working, health),
            "Suspended" => Health::new(Level::Attention, health),
            other => Health::new(Level::Unknown, other),
        };
    }

    let conditions = object.array_at("status.conditions");
    let condition_is = |kind: &str, status: &str| {
        conditions.iter().any(|condition| {
            condition.get("type").and_then(Value::as_str) == Some(kind)
                && condition.get("status").and_then(Value::as_str) == Some(status)
        })
    };
    if condition_is("ErrorOccurred", "True") || condition_is("ResourcesUpToDate", "False") {
        Health::new(Level::Error, "Degraded")
    } else if condition_is("RolloutProgressing", "True") {
        Health::new(Level::Working, "Progressing")
    } else if condition_is("ResourcesUpToDate", "True") {
        Health::new(Level::Ok, "Healthy")
    } else {
        Health::new(Level::Working, "Progressing")
    }
}

/// One of the enumerated phases, or unknown.
fn phase(object: &Object, table: &[(&str, Level)]) -> Health {
    let phase = object.str_at("status.phase");
    match table.iter().find(|(name, _)| *name == phase) {
        Some((name, level)) => Health::new(*level, *name),
        None if phase.is_empty() => Health::unknown(),
        None => Health::new(Level::Unknown, phase),
    }
}

/// What a kind with no rule of its own gets.
///
/// `Ready` and `Available` are the two condition types the community has
/// converged on, and a controller that sets neither is one we genuinely have
/// nothing to say about.
fn fallback(object: &Object) -> Health {
    for name in ["Ready", "Available"] {
        if object.condition_is(name, "True") {
            return Health::new(Level::Ok, name);
        }
        if object.condition_is(name, "False") {
            let reason = object
                .condition(name)
                .and_then(|condition| condition.get("reason"))
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
                .unwrap_or(name);
            return Health::new(Level::Error, reason);
        }
    }
    Health::unknown()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: serde_json::Value) -> Object {
        let mut value = value;
        if value.get("metadata").is_none() {
            value["metadata"] = json!({"name": "x"});
        }
        Object::new(value).unwrap()
    }

    fn health(kind: &str, value: serde_json::Value) -> Health {
        of(kind, &object(value))
    }

    #[test]
    fn a_running_pod_with_every_container_ready_is_the_only_green_one() {
        let ready = health(
            "Pod",
            json!({"status": {"phase": "Running", "containerStatuses": [
                {"ready": true, "restartCount": 0}, {"ready": true, "restartCount": 1}
            ]}}),
        );
        assert_eq!(ready, Health::new(Level::Ok, "Running"));

        let short = health(
            "Pod",
            json!({"status": {"phase": "Running", "containerStatuses": [
                {"ready": true}, {"ready": false}
            ]}}),
        );
        assert_eq!(short.level, Level::Attention);
        assert_eq!(short.word, "Running");
    }

    #[test]
    fn a_crash_looping_pod_still_says_running_in_its_phase_and_must_not_be_green() {
        let health = health(
            "Pod",
            json!({"status": {"phase": "Running", "containerStatuses": [
                {"ready": false, "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
            ]}}),
        );
        assert_eq!(health, Health::new(Level::Error, "CrashLoopBackOff"));
    }

    #[test]
    fn a_container_being_created_is_working_and_not_broken() {
        let health = health(
            "Pod",
            json!({"status": {"phase": "Pending", "containerStatuses": [
                {"ready": false, "state": {"waiting": {"reason": "ContainerCreating"}}}
            ]}}),
        );
        assert_eq!(health, Health::new(Level::Working, "ContainerCreating"));
    }

    #[test]
    fn an_init_container_that_cannot_pull_is_the_reason_and_not_pod_initializing() {
        let health = health(
            "Pod",
            json!({"status": {"phase": "Pending",
            "initContainerStatuses": [
                {"state": {"waiting": {"reason": "ImagePullBackOff"}}}
            ],
            "containerStatuses": [
                {"state": {"waiting": {"reason": "PodInitializing"}}}
            ]}}),
        );
        assert_eq!(health, Health::new(Level::Error, "ImagePullBackOff"));
    }

    #[test]
    fn a_finished_pod_says_completed_the_way_kubectl_does() {
        assert_eq!(
            health("Pod", json!({"status": {"phase": "Succeeded"}})),
            Health::new(Level::Ok, "Completed")
        );
    }

    #[test]
    fn a_pod_whose_node_is_lost_is_an_error_and_not_an_absence() {
        assert_eq!(
            health("Pod", json!({"status": {"phase": "Unknown"}})).level,
            Level::Error
        );
    }

    #[test]
    fn a_container_that_exited_non_zero_is_an_error_even_with_nothing_waiting() {
        let health = health(
            "Pod",
            json!({"status": {"phase": "Running", "containerStatuses": [
                {"ready": false, "state": {"terminated": {"exitCode": 137, "reason": "OOMKilled"}}}
            ]}}),
        );
        assert_eq!(health, Health::new(Level::Error, "OOMKilled"));
    }

    #[test]
    fn a_pod_being_deleted_is_terminating_whatever_else_it_says() {
        let value = json!({
            "metadata": {"name": "x", "deletionTimestamp": "2026-09-07T00:00:00Z"},
            "status": {"phase": "Running", "containerStatuses": [
                {"ready": false, "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
            ]}
        });
        assert_eq!(
            health("Pod", value),
            Health::new(Level::Working, "Terminating")
        );
    }

    #[test]
    fn ready_counts_fall_back_to_the_spec_before_the_kubelet_reports() {
        let pending = object(json!({"spec": {"containers": [{"name": "a"}, {"name": "b"}]}}));
        assert_eq!(ready_containers(&pending), (0, 2));
        let running = object(json!({
            "spec": {"containers": [{"name": "a"}, {"name": "b"}]},
            "status": {"containerStatuses": [{"ready": true}, {"ready": false}]}
        }));
        assert_eq!(ready_containers(&running), (1, 2));
    }

    #[test]
    fn restarts_are_summed_across_containers() {
        let pod = object(json!({"status": {"containerStatuses": [
            {"restartCount": 3}, {"restartCount": 4}
        ]}}));
        assert_eq!(restarts(&pod), 7);
    }

    #[test]
    fn a_cordoned_node_is_never_green() {
        let cordoned = health(
            "Node",
            json!({"spec": {"unschedulable": true},
                   "status": {"conditions": [{"type": "Ready", "status": "True"}]}}),
        );
        assert_eq!(cordoned.level, Level::Attention);
        assert!(cordoned.word.contains("SchedulingDisabled"));
    }

    #[test]
    fn a_node_under_pressure_is_ready_and_still_worth_looking_at() {
        let health = health(
            "Node",
            json!({"status": {"conditions": [
                {"type": "Ready", "status": "True"},
                {"type": "DiskPressure", "status": "True"}
            ]}}),
        );
        assert_eq!(health, Health::new(Level::Attention, "DiskPressure"));
    }

    #[test]
    fn a_node_the_control_plane_has_lost_touch_with_is_an_error() {
        assert_eq!(
            health(
                "Node",
                json!({"status": {"conditions": [{"type": "Ready", "status": "Unknown"}]}})
            )
            .level,
            Level::Error
        );
        assert_eq!(
            health(
                "Node",
                json!({"status": {"conditions": [{"type": "Ready", "status": "False"}]}})
            ),
            Health::new(Level::Error, "NotReady")
        );
    }

    #[test]
    fn a_controller_reports_ready_over_desired() {
        assert_eq!(
            health(
                "Deployment",
                json!({"spec": {"replicas": 3}, "status": {"readyReplicas": 3}})
            ),
            Health::new(Level::Ok, "3/3")
        );
        assert_eq!(
            health(
                "Deployment",
                json!({"spec": {"replicas": 3}, "status": {"readyReplicas": 1}})
            ),
            Health::new(Level::Attention, "1/3")
        );
        assert_eq!(
            health("Deployment", json!({"spec": {"replicas": 3}, "status": {}})),
            Health::new(Level::Error, "0/3")
        );
    }

    #[test]
    fn a_deployment_scaled_to_zero_is_neither_healthy_nor_broken() {
        assert_eq!(
            health("Deployment", json!({"spec": {"replicas": 0}, "status": {}})),
            Health::new(Level::Unknown, "0/0")
        );
    }

    #[test]
    fn a_deployment_with_no_replica_count_wants_one() {
        assert_eq!(
            health(
                "Deployment",
                json!({"spec": {}, "status": {"readyReplicas": 1}})
            ),
            Health::new(Level::Ok, "1/1")
        );
    }

    #[test]
    fn a_daemonset_counts_what_the_scheduler_wanted() {
        assert_eq!(
            health(
                "DaemonSet",
                json!({"status": {"desiredNumberScheduled": 3, "numberReady": 3}})
            ),
            Health::new(Level::Ok, "3/3")
        );
    }

    #[test]
    fn a_job_is_read_from_its_conditions_first() {
        assert_eq!(
            health(
                "Job",
                json!({"status": {"conditions": [{"type": "Failed", "status": "True"}]}})
            ),
            Health::new(Level::Error, "Failed")
        );
        assert_eq!(
            health(
                "Job",
                json!({"spec": {"completions": 1}, "status": {"succeeded": 1}})
            ),
            Health::new(Level::Ok, "1/1")
        );
        assert_eq!(
            health(
                "Job",
                json!({"spec": {"completions": 3}, "status": {"active": 2}})
            )
            .level,
            Level::Working
        );
    }

    #[test]
    fn a_suspended_cronjob_is_never_silent() {
        assert_eq!(
            health("CronJob", json!({"spec": {"suspend": true}})),
            Health::new(Level::Attention, "Suspended")
        );
        assert_eq!(health("CronJob", json!({"spec": {}})).level, Level::Ok);
    }

    #[test]
    fn an_argo_application_combines_operation_health_and_sync() {
        let argo_health = |value| {
            of_resource(
                &ResourceKey::new("argoproj.io", "Application"),
                &object(value),
            )
        };
        assert_eq!(
            argo_health(json!({"status": {"sync": {"status": "Synced"},
                                           "health": {"status": "Healthy"}}})),
            Health::new(Level::Ok, "Synced")
        );
        assert_eq!(
            argo_health(json!({"status": {"sync": {"status": "OutOfSync"},
                                           "health": {"status": "Healthy"}}})),
            Health::new(Level::Attention, "OutOfSync")
        );
        assert_eq!(
            argo_health(json!({"status": {"sync": {"status": "Synced"},
                                           "health": {"status": "Degraded"}}})),
            Health::new(Level::Error, "Degraded")
        );
        assert_eq!(
            argo_health(json!({"status": {"operationState": {"phase": "Running"},
                                           "sync": {"status": "OutOfSync"}}})),
            Health::new(Level::Working, "Running")
        );
        assert_eq!(
            of_resource(
                &ResourceKey::new("example.com", "Application"),
                &object(json!({"status": {"sync": {"status": "OutOfSync"}}})),
            ),
            Health::unknown()
        );
    }

    #[test]
    fn an_argo_application_set_uses_argos_condition_priority() {
        let appset_health = |value| {
            of_resource(
                &ResourceKey::new("argoproj.io", "ApplicationSet"),
                &object(value),
            )
        };
        assert_eq!(
            appset_health(json!({"status": {"health": {"status": "Healthy"}}})),
            Health::new(Level::Ok, "Healthy")
        );
        assert_eq!(
            appset_health(json!({"status": {"conditions": [
                {"type": "ResourcesUpToDate", "status": "True"},
                {"type": "ErrorOccurred", "status": "True"}
            ]}})),
            Health::new(Level::Error, "Degraded")
        );
        assert_eq!(
            appset_health(json!({"status": {"conditions": [
                {"type": "ResourcesUpToDate", "status": "False"}
            ]}})),
            Health::new(Level::Error, "Degraded")
        );
        assert_eq!(
            appset_health(json!({"status": {"conditions": [
                {"type": "RolloutProgressing", "status": "True"},
                {"type": "ResourcesUpToDate", "status": "True"}
            ]}})),
            Health::new(Level::Working, "Progressing")
        );
        assert_eq!(
            appset_health(json!({"status": {"conditions": [
                {"type": "ResourcesUpToDate", "status": "True"}
            ]}})),
            Health::new(Level::Ok, "Healthy")
        );
    }

    #[test]
    fn a_claim_reports_its_phase() {
        assert_eq!(
            health(
                "PersistentVolumeClaim",
                json!({"status": {"phase": "Bound"}})
            ),
            Health::new(Level::Ok, "Bound")
        );
        assert_eq!(
            health(
                "PersistentVolumeClaim",
                json!({"status": {"phase": "Pending"}})
            )
            .level,
            Level::Working
        );
        assert_eq!(
            health(
                "PersistentVolumeClaim",
                json!({"status": {"phase": "Lost"}})
            )
            .level,
            Level::Error
        );
    }

    #[test]
    fn a_load_balancer_with_no_address_yet_is_working() {
        assert_eq!(
            health(
                "Service",
                json!({"spec": {"type": "LoadBalancer"}, "status": {}})
            ),
            Health::new(Level::Working, "Pending")
        );
        assert_eq!(
            health(
                "Service",
                json!({"spec": {"type": "LoadBalancer"},
                       "status": {"loadBalancer": {"ingress": [{"ip": "10.0.0.9"}]}}})
            ),
            Health::new(Level::Ok, "LoadBalancer")
        );
        assert_eq!(
            health("Service", json!({"spec": {"type": "ClusterIP"}})),
            Health::new(Level::Ok, "ClusterIP")
        );
    }

    #[test]
    fn a_custom_resource_gets_a_mark_without_a_rule_being_written_for_it() {
        assert_eq!(
            health(
                "Rollout",
                json!({"status": {"conditions": [{"type": "Available", "status": "True"}]}})
            ),
            Health::new(Level::Ok, "Available")
        );
        assert_eq!(
            health(
                "Rollout",
                json!({"status": {"conditions": [
                    {"type": "Ready", "status": "False", "reason": "ProgressDeadlineExceeded"}
                ]}})
            ),
            Health::new(Level::Error, "ProgressDeadlineExceeded")
        );
    }

    #[test]
    fn a_kind_with_nothing_to_read_says_nothing_rather_than_guessing() {
        assert_eq!(
            health("ConfigMap", json!({"data": {"a": "b"}})),
            Health::unknown()
        );
        assert_eq!(Health::unknown().level.token(), "text.muted");
    }

    #[test]
    fn only_the_two_levels_worth_an_alarm_count_as_bad() {
        assert!(Level::Error.is_bad());
        assert!(Level::Attention.is_bad());
        assert!(!Level::Ok.is_bad());
        assert!(!Level::Working.is_bad());
        assert!(!Level::Unknown.is_bad());
    }
}
