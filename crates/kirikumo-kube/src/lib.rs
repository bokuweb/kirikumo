//! The Kubernetes domain: everything about clusters that does not draw.
//!
//! What lives here: reading and merging a kubeconfig ([`kubeconfig`]), turning
//! a context into credentials that survive rotation ([`auth`]), asking an
//! apiserver what it serves ([`discovery`]) and what columns its custom
//! kinds declare ([`crd`], evaluated by [`jsonpath`]), the generic object
//! model ([`model`]), the rule that turns an object into a health mark
//! ([`health`]), quantity parsing ([`quantity`]), the [`Cluster`] trait every
//! view reaches a cluster through, including the API-group-aware identity an
//! optional GitOps presentation uses, its REST implementation ([`rest`]), the
//! framing of a watch ([`watch`]), the few writes and what each becomes on
//! the wire ([`actions`], [`drain`]), a port forwarded to a pod and a command
//! run in one ([`portforward`], [`exec`], over [`tls`]), and a scripted fake
//! ([`scripted`]).
//!
//! What does not live here: anything that knows a colour, a column or a
//! window. This crate has no `gpui` dependency and never will
//! (`AGENTS.md` rule 6).
//!
//! Two constraints shape all of it. Objects are `serde_json::Value` plus a
//! parsed [`ObjectMeta`], never generated types, so a custom resource costs
//! nothing (rule 8). And every call is *blocking* and `Send + Sync`, because
//! the host that will one day own this window runs on `smol` and forbids a
//! second async runtime (rule 3).

pub mod actions;
pub mod auth;
pub mod crd;
pub mod discovery;
pub mod drain;
pub mod error;
pub mod exec;
pub mod health;
pub mod jsonpath;
pub mod kubeconfig;
pub mod logs;
pub mod model;
pub mod portforward;
pub mod quantity;
pub mod rest;
pub mod scripted;
pub mod tls;
pub mod watch;
pub mod yaml;

pub use actions::Action;
pub use crd::{PrinterColumn, PrinterColumns};
pub use error::{Error, Result};
pub use exec::{ExecOutput, ExecRequest};
pub use health::{Health, Level};
pub use kubeconfig::{KubeConfig, kubeconfig_paths};
pub use logs::LogStream;
pub use model::{
    ApiResource, Catalogue, ClusterVersion, ContextRef, EventRecord, Group, LogRequest, Metrics,
    Object, ObjectList, ObjectMeta, OwnerRef, Patch, ResourceKey,
};
pub use portforward::{Forwarder, Tunnel};
pub use rest::Rest;
pub use scripted::Scripted;
pub use watch::{Applied, WatchEvent, WatchStream};

/// Everything a view may ask of a cluster.
///
/// The one path from a view to the network (`AGENTS.md` rule 2). Blocking and
/// `Send + Sync` on purpose: the standalone app calls it on GPUI's background
/// executor, and the host that will mount these views implements it over its
/// own RPC with a `block_on`. An async trait would fix the executor for both.
///
/// Every method a cluster might not offer — metrics, watches, writes — has a
/// default that answers [`Error::Unsupported`], so a partial implementation
/// still compiles and a cluster that refuses degrades to a viewer rather than
/// to an error page.
pub trait Cluster: Send + Sync {
    /// What the apiserver says it is.
    fn version(&self) -> Result<ClusterVersion>;

    /// Everything the cluster serves, one preferred version per kind.
    fn catalogue(&self) -> Result<Catalogue>;

    /// Every namespace, for the namespace picker.
    fn namespaces(&self) -> Result<Vec<String>>;

    /// Every object of one kind, in one namespace or across all of them.
    ///
    /// `namespace` is ignored for cluster-scoped resources.
    fn list(&self, resource: &ApiResource, namespace: Option<&str>) -> Result<ObjectList>;

    /// One object.
    fn get(&self, resource: &ApiResource, namespace: Option<&str>, name: &str) -> Result<Object>;

    /// The events an object is involved in, newest first.
    fn events_for(&self, uid: &str, namespace: Option<&str>) -> Result<Vec<EventRecord>>;

    /// A container's log, as text.
    fn logs(&self, request: &LogRequest) -> Result<String>;

    /// Follow a container's log.
    ///
    /// The stream is read on a thread of its own and ends when the container
    /// does, which is not an error (see [`logs`]).
    fn follow_logs(&self, _request: &LogRequest) -> Result<Box<dyn LogStream>> {
        Err(Error::Unsupported)
    }

    /// Node resource use, when `metrics.k8s.io` is installed.
    fn node_metrics(&self) -> Result<Vec<Metrics>> {
        Err(Error::Unsupported)
    }

    /// Pod resource use, when `metrics.k8s.io` is installed.
    fn pod_metrics(&self, _namespace: Option<&str>) -> Result<Vec<Metrics>> {
        Err(Error::Unsupported)
    }

    /// Follow a kind from a known `resourceVersion`.
    ///
    /// The stream is read on a thread of its own; see [`watch`] for the
    /// framing and for what a `410 Gone` means.
    fn watch(
        &self,
        _resource: &ApiResource,
        _namespace: Option<&str>,
        _from: &str,
    ) -> Result<Box<dyn WatchStream>> {
        Err(Error::Unsupported)
    }

    /// Delete one object. A write: see `AGENTS.md` rule 9.
    fn delete(&self, _resource: &ApiResource, _namespace: Option<&str>, _name: &str) -> Result<()> {
        Err(Error::Unsupported)
    }

    /// Patch one object. A write: see `AGENTS.md` rule 9.
    fn patch(
        &self,
        _resource: &ApiResource,
        _namespace: Option<&str>,
        _name: &str,
        _patch: Patch,
    ) -> Result<Object> {
        Err(Error::Unsupported)
    }

    /// Create one Job immediately from a CronJob's job template.
    ///
    /// Narrow by design: Kirikumo is not a generic object-authoring client,
    /// and the permission for this operation is `create` on `batch/Job`, not
    /// a write permission on the CronJob being read.
    fn trigger_cron_job(&self, _resource: &ApiResource, _cron_job: &Object) -> Result<Object> {
        Err(Error::Unsupported)
    }

    /// Open one connection to a port on a pod.
    ///
    /// Called once per local connection by [`portforward::Forwarder`], on
    /// that connection's thread; see [`portforward`] for why it is one
    /// tunnel per connection.
    fn port_forward(&self, _namespace: &str, _pod: &str, _port: u16) -> Result<Box<dyn Tunnel>> {
        Err(Error::Unsupported)
    }

    /// Run a command in a container and collect what it wrote.
    ///
    /// Non-interactive: no stdin, no tty, the whole output at the end. Slow
    /// by nature — it lasts as long as the command — so it belongs on the
    /// background executor like every other call.
    fn exec(&self, _request: &ExecRequest) -> Result<ExecOutput> {
        Err(Error::Unsupported)
    }

    /// Attach to a shell in a container: a tty, stdin open, and a tunnel
    /// whose bytes are the terminal's.
    ///
    /// The tunnel is polled on a thread of its own, like a port-forward's;
    /// what is typed goes back down it, and a resize goes on its own channel.
    fn attach(&self, _request: &ExecRequest) -> Result<Box<dyn Tunnel>> {
        Err(Error::Unsupported)
    }

    /// Ask a pod to leave, through the Eviction API.
    ///
    /// Unlike a delete this honours a `PodDisruptionBudget`: when the budget
    /// says no the apiserver answers `429`, which arrives as
    /// [`Error::Api`] with that status, and [`drain`] waits and asks again.
    fn evict(&self, _namespace: &str, _name: &str) -> Result<()> {
        Err(Error::Unsupported)
    }

    /// Whether this client may do something, per `SelfSubjectAccessReview`.
    ///
    /// Answering `false` greys a control out rather than letting the reader
    /// press it and read a 403. An implementation that cannot ask says `true`
    /// and lets the apiserver be the judge.
    fn can_i(
        &self,
        _resource: &ApiResource,
        _namespace: Option<&str>,
        _verb: &str,
    ) -> Result<bool> {
        Ok(true)
    }
}
