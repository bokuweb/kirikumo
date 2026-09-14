# Kirikumo Roadmap

> The authoritative document for this repository. `AGENTS.md` is the short version; `docs/ui.md` says what it looks like.
> Last updated: 2026-09-14

## 1. Vision

**Kirikumo is a native Kubernetes viewer, written in Rust on GPUI, built to stand on its own and to be mounted inside [Ginka](https://github.com/bokuweb/ginka).**

It answers the questions a person opens Lens for — what is running, what is unhealthy, what did that pod log, what does this object actually look like — in one window, over any cluster their kubeconfig can reach, with no agent installed in the cluster and no account anywhere.

It is the third window in a family that becomes one application:

| Repository | What it shows | What it becomes |
| --- | --- | --- |
| [`ginka`](https://github.com/bokuweb/ginka) | coding agents across worktrees | the host: the window, the daemon, the theme |
| [`e1`](https://github.com/bokuweb/e1) | GitHub: inbox, pulls, issues, diffs | a surface in that window |
| **`kirikumo`** | Kubernetes: workloads, config, logs | a surface in that window |

So every decision here is made twice: once for the standalone window, and once for the day these three are one binary. §4.3 is where that second answer is written down, and it is the reason this repository looks the way it does.

## 2. What we take from the references

- **[Lens](https://k8slens.dev/)** — the shape of the thing: a cluster picked at the top, a resource tree grouped into Workloads / Config / Network / Storage / Access Control / Custom Resources, a table in the middle, an object drawer with Overview, Events, YAML and Logs.
- **[k9s](https://k9scli.io/)** — its discipline about *what a row is worth*: the columns are the ones `kubectl get` prints, health is one mark at the head of the row, and everything is one keystroke away.
- **[Headlamp](https://headlamp.dev/)** — generic-first rendering: the resource catalogue comes from API discovery, so a CRD gets a table without anyone writing code for it.
- **`kubectl`** — the wire behaviour: discovery, `?watch=true` bookmarks and `410 Gone`, `metrics.k8s.io` being optional, exec credential plugins.

**Kirikumo is written here.** These are read for behaviour and for the edge cases they have already hit; no code is copied, ported or paraphrased in. Lens is closed-source above its core, k9s and Headlamp are Apache-2.0, and the rule holds for all of them: an implementation we did not write is one we cannot debug.

## 3. Scope

### 3.1 v1.0 definition of done

1. Open on the context the kubeconfig says is current, and switch between contexts without restarting.
2. List **any** resource the cluster serves, including custom resources, from API discovery — not a hardcoded list.
3. A table per kind with the columns `kubectl get` prints, a health mark per row, a namespace filter and a fuzzy filter over the rows.
4. An object detail with Overview, Events, YAML, and — for anything with containers — Logs, following.
5. Live: what is on screen is watched, and a change in the cluster reaches the row without a refresh.
6. Node and pod resource use, when `metrics.k8s.io` is installed, and silence when it is not.
7. The destructive actions a viewer still needs — delete, scale, restart, cordon — behind a confirmation and greyed out when RBAC says no.
8. Read-only by default over any auth a kubeconfig can express: client certificates, tokens, token files and exec credential plugins.

### 3.2 Non-goals for v1

- **No cluster install.** Nothing is deployed into the cluster to make this work — no agent, no metrics collector, no dashboards.
- **No editor.** YAML is shown and can be applied, but authoring manifests is what an editor is for. (`kubectl apply` from a file, and Ginka's own code surface, are the answer.)
- **No Helm, no charts, no marketplace.** Lens's chart catalogue is a package manager wearing a viewer's clothes.
- **No multi-cluster aggregation.** One context at a time; switching is cheap.
- **No metrics history and no Prometheus.** Instantaneous use from `metrics.k8s.io`, nothing stored.
- **No terminal into the machine.** A shell into a *container* is there (M5); a shell on a node, or on the desktop, is what Ginka's terminal dock is for.

## 4. Architecture

### 4.1 Process model

One process, and no daemon of our own. The cluster is the remote and the kubeconfig is the only state we read; the window holds view state and in-memory caches, so closing it loses nothing.

```
┌─────────────────────────────────────────────────┐  HTTPS   ┌──────────────────┐
│  kirikumo (GPUI app)                            │  (ureq,  │  kube-apiserver  │
│  src/main.rs        opens the window            │◄────────►│                  │
│  kirikumo-views     Shell / Sidebar / Table /   │  mTLS or │  /api, /apis     │
│                     Detail, over Arc<dyn Cluster>│  bearer │  ?watch=true     │
│  kirikumo-ui        tokens, settings, view models│         │  metrics.k8s.io  │
│  kirikumo-kube      Cluster trait, kubeconfig,  │         └──────────────────┘
│                     discovery, REST, Scripted   │
└─────────────────────────────────────────────────┘
```

Every request is blocking and runs on GPUI's background executor. A watch is a blocking read of a chunked response on a thread of its own, which posts each event back to the window; that is the same shape a driver's reader has in Ginka, and it is why no async runtime appears in the graph (§4.3, K3).

When embedded, the same views sit in Ginka's window and the `Arc<dyn Cluster>` they are given proxies through Ginka's daemon, so Ginka's rule that the daemon owns all state holds without any view knowing.

### 4.2 Crate layout

```
kirikumo/
├─ Cargo.toml              # workspace root; the `kirikumo` binary, deliberately thin
├─ src/main.rs             # paths, settings, locale, logging, the window, Shell
├─ crates/
│  ├─ kirikumo-kube/       # kubeconfig, auth, discovery, the model, the Cluster
│  │                       # trait, the REST client, watches, the Scripted fake.
│  │                       # No GPUI.
│  ├─ kirikumo-ui/         # Tokens + theme apply, Assets, Layout, AppSettings,
│  │                       # Paths, i18n, logging, the sidebar tree, the table's
│  │                       # columns, row filtering, Fetch
│  └─ kirikumo-views/      # Store, Shell, Sidebar, ResourceTable, Detail. A
│                          # library, with no tests (AGENTS.md rule 6).
├─ locales/app.yml
├─ assets/themes/{dark,light}.json     # Ginka's tokens, byte for byte
├─ assets/icons/*.svg
└─ docs/{roadmap,ui}.md
```

Dependency direction: `kirikumo-views → kirikumo-ui → kirikumo-kube`. `kirikumo-kube` knows nothing about the UI; `kirikumo-ui` knows the model but not the views; `kirikumo-views` is the only crate that touches `gpui-component`'s render chains.

### 4.3 The embedding contract

Ginka will add `kirikumo-views` to its workspace and mount `kirikumo_views::ClusterPanel` — the table and the detail, without the window's own chrome — as a surface in its right panel, with the resource tree folded into its sidebar. For that to be a mount rather than a rewrite, six things are held constant from now on. They are e1's E1–E5 with one addition, and they are numbered K so the two lists can be compared line by line.

| # | Constraint | Why it is decided now |
| --- | --- | --- |
| K1 | **Views are a library crate.** `src/main.rs` opens a window and nothing more. | A view in a binary cannot be linked. |
| K2 | **`Arc<dyn Cluster>` is the only way a view reaches a cluster.** The trait is blocking, `Send + Sync`, and called on the background executor. | Ginka's daemon owns state; its implementation will answer over its RPC. A blocking trait can be implemented over a channel with `block_on`; an async trait would fix the executor. |
| K3 | **One reactor by default; a second one is a recorded decision, confined behind `Cluster`.** Today that means `ureq` over rustls, a watch on a thread of its own, and no tokio in the graph at all. | Ginka's own rule is that a second runtime needs an entry in its decision log — not that it is forbidden. Two runtimes coexist perfectly well as separate thread pools; the hazard is narrower and nastier than that. A `hyper` future polled from `smol` panics at *run* time rather than failing to compile, so whichever runtime a client needs has to be sealed inside the implementation of the trait. `Cluster` is where that seal is, which is what keeps this reversible. See §8 and Q2. |
| K4 | **`gpui-component` and `gpui` at Ginka's locked revs**, and no other GPUI library. | Two revs of `gpui` are two unrelated `App`, `Window`, `Element` types. `Cargo.lock` was seeded from e1's, which was seeded from Ginka's. |
| K5 | **The token schema is Ginka's.** `kirikumo_ui::Tokens` deserialises the same JSON, and `assets/themes/*.json` is Ginka's file unchanged. | A view written against `text.secondary` renders correctly under any of the three apps' themes. |
| K6 | **Nothing writes to a cluster without an explicit, per-action confirmation, and no write is reachable from a key chord alone.** | The other two apps act on a working copy; this one acts on production. When all three are one window, a muscle-memory keystroke must not be able to delete a StatefulSet. |

What is *not* held constant: the window's own header strips, its traffic-light inset, the appearance control and settings persistence. Those live in `Shell`, and `Shell` is the one view Ginka will not mount.

### 4.4 Data model

Kubernetes already has a data model, and it is `unstructured`: every object is JSON with `apiVersion`, `kind`, `metadata`, and whatever else its schema says. Typing each kind here would mean a code change for every CRD, which is exactly what §2 says not to do. So the model is thin and generic, in `kirikumo_kube::model`:

| Type | What it is | Where it comes from |
| --- | --- | --- |
| `ContextRef` | one entry in the kubeconfig: name, cluster, user, default namespace | `~/.kube/config`, or every file in `KUBECONFIG` |
| `ClusterAccess` | what it takes to reach one cluster: server URL, roots, client cert or token, and how to refresh it | a `ContextRef` resolved against its cluster and user |
| `ApiResource` | one thing the cluster serves: group, version, kind, plural name, namespaced or not, verbs, short names, categories | `/api`, `/apis`, `/apis/{group}/{version}` |
| `Catalogue` | every `ApiResource`, deduplicated to one preferred version per kind, sorted into the sidebar's groups | discovery, once per connection |
| `Object` | one resource: the raw JSON, plus the `ObjectMeta` every object has | any list or get |
| `ObjectMeta` | name, namespace, uid, resourceVersion, creation time, labels, annotations, owner references | `metadata`, which is the one schema every kind shares |
| `ObjectList` | a page of objects and the `resourceVersion` a watch continues from | a list |
| `Health` | one of `Ok`, `Working`, `Attention`, `Error`, `Unknown`, with a word for it | computed per kind in `kirikumo_kube::health` |
| `WatchEvent` | `Added`, `Modified`, `Deleted`, `Bookmark`, or `Error` with a status | `?watch=true`, one JSON object per line |
| `EventRecord` | one `v1.Event`: type, reason, message, count, when | `/api/v1/events?fieldSelector=involvedObject.uid=…` |
| `Metrics` | CPU in milli-cores and memory in bytes, per node or per pod | `metrics.k8s.io/v1beta1` |
| `ClusterVersion` | what the apiserver says it is | `/version` |

`Object` keeps the raw `serde_json::Value`, and everything drawn from it — a column, a health mark, a detail row — is a function of that value written once and tested against real payloads. This is the single most important shape in the repository: it is what makes a CRD free.

### 4.5 The `Cluster` trait

```rust
pub trait Cluster: Send + Sync {
    fn version(&self) -> Result<ClusterVersion>;
    fn catalogue(&self) -> Result<Catalogue>;
    fn namespaces(&self) -> Result<Vec<String>>;
    fn list(&self, resource: &ApiResource, namespace: Option<&str>) -> Result<ObjectList>;
    fn get(&self, resource: &ApiResource, namespace: Option<&str>, name: &str) -> Result<Object>;
    fn events_for(&self, uid: &str, namespace: Option<&str>) -> Result<Vec<EventRecord>>;
    fn logs(&self, request: &LogRequest) -> Result<String>;
    fn node_metrics(&self) -> Result<Vec<Metrics>>;   // defaults to Unsupported
    fn pod_metrics(&self, namespace: Option<&str>) -> Result<Vec<Metrics>>;  // ditto
    fn watch(&self, resource: &ApiResource, namespace: Option<&str>, from: &str)
        -> Result<Box<dyn WatchStream>>;              // ditto
    fn delete(&self, resource: &ApiResource, namespace: Option<&str>, name: &str) -> Result<()>;
    fn patch(&self, resource: &ApiResource, namespace: Option<&str>, name: &str,
             patch: Patch) -> Result<Object>;         // both default to Unsupported
    fn trigger_cron_job(&self, resource: &ApiResource, cron_job: &Object)
        -> Result<Object>;                            // defaults to Unsupported
    fn can_i(&self, resource: &ApiResource, namespace: Option<&str>, verb: &str) -> Result<bool>;
}
```

Small on purpose: every method is one screen's question, and a `list` that takes an `ApiResource` is what lets one table draw every kind. Writes arrive in M4 as methods with a default `Err(Unsupported)`, so a host implementation that cannot do them yet still compiles — and so a cluster that refuses them degrades to a viewer rather than to an error.

Two implementations ship: `Rest` (ureq over a per-cluster agent carrying the kubeconfig's TLS) and `Scripted` (in-memory, with a sample cluster used by tests and by `KIRIKUMO_DEMO=1`).

### 4.6 Kubeconfig and authentication

`KUBECONFIG` is a `:`-separated list and it *merges*: the first file to name a context, cluster or user wins, and `current-context` comes from the first file that sets one. With no `KUBECONFIG`, `~/.kube/config`. `KIRIKUMO_KUBECONFIG` overrides both, which is how a test keeps out of the real one.

A context resolves to a `ClusterAccess` carrying one of:

- **Client certificates** — `client-certificate[-data]` and `client-key[-data]`, PEM, handed to `ureq` as a `ClientCert`. This is what `kind`, `minikube` and most bare clusters use.
- **A bearer token** — `token`, or `tokenFile` re-read on every request because a projected service-account token rotates.
- **An exec credential plugin** — `user.exec`, the way EKS, GKE and AKS all authenticate now: run the command, parse the `ExecCredential` it prints, use its `status.token` or its client certificate, and re-run it when `expirationTimestamp` has passed. The command is run with its `env` added to ours, never with a shell.
- **Basic auth** — `username`/`password`, still present in old files, supported and never stored.

The server's roots come from `certificate-authority[-data]`; `insecure-skip-tls-verify: true` is honoured because a homelab cluster is a real thing, and the window says so in the header when it is on, because a viewer that hides that is worse than one that refuses.

Nothing is written back: this app never edits a kubeconfig, and never keeps a credential on disk. A token an exec plugin produced lives in memory until it expires.

### 4.7 Watching, and what is cached

There is no disk cache. Kubernetes answers are large, short-lived and often secret, and `304` is not part of the apiserver's vocabulary the way it is GitHub's; a `store.json` full of pod specs would be a liability with no speed to show for it. What we keep is in memory, for as long as the window is open.

A watch is the mechanism, not a refresh loop:

1. `list` a kind, note the `resourceVersion` of the list.
2. `GET …?watch=true&resourceVersion=…&allowWatchBookmarks=true` and read newline-delimited JSON on a thread of its own.
3. Apply each event to the table's rows, and remember the bookmark's version so a reconnect resumes where we were.
4. On `410 Gone` — the version has aged out — list again and start over. On a transport failure, back off (1 s, doubling to 30 s) and retry.

Only what is on screen is watched, and a watch is dropped when its table is. A cluster with ten thousand pods must not be streamed because the sidebar mentions pods.

### 4.8 UI stack

`gpui-component` over the `gpui` rev it owns, at exactly Ginka's and e1's lock (K4), with its `tree-sitter-yaml` feature on. Used from it: `Root`, `Icon`, `Input`, `Tooltip`, `Editor` for the YAML tab, and `gpui::uniform_list` for every table. Built here: the header strips, the health marks, the table's own header and cells, the log view, the palette, the pickers.

Before writing a widget, check `gpui-component`'s gallery for an existing one.

## 5. Milestones

**M0–M5 have landed.** What works today:

- The **window** opens frameless over a blurred desktop, with the three columns resizable and their arrangement, the appearance, the context, the kind and the namespace all remembered across launches.
- The **kubeconfig layer** reads and merges `KUBECONFIG` first-wins, resolves a context into a connection, and authenticates with client certificates, a token, a token file, basic auth or an **exec credential plugin** whose answer is cached until it expires.
- **Discovery** builds the sidebar: every listable kind the apiserver serves, one preferred version per kind, sorted into the standard groups, with Argo CD in an optional GitOps group and all other custom resources under their API group. Nothing disappears when it is unknown.
- **One table draws every kind**, virtualized, with `kubectl get`'s columns per kind and Name/Namespace/Age for everything else, a health mark per row, sortable headings, a namespace picker and a fuzzy filter over every cell and label.
- The **detail panel** has Overview (per-kind facts plus conditions), Events, YAML (rendered locally from the object already on screen) and, for anything with containers, Logs with a container picker.
- **The table is live and configurable.** The list on screen — and only that one — is watched: a thread reads the stream, bookmarks keep the resume point moving while nothing happens, a `410 Gone` re-lists and starts again, and a dropped connection backs off from a second to thirty. A change reaches the row without anybody pressing refresh, and only the rows whose objects moved are formatted again. Its columns can be resized, reordered and hidden per resource without persisting any row data.
- **A log can be followed.** The Logs tab tails a container and keeps reading, with a container picker, a *Previous* toggle for the instance before this one, and a literal find that filters and counts. A workload with a pod template and selector gets the same tab over its matching Pods, newest first, with an explicit Pod picker. A Node gets it over the Pods scheduled there, across namespaces; this is the honest meaning of “Node logs” because Kubernetes has no Node logs subresource. Run and Shell stay Pod-only. The scrollback is bounded; the reading is a thread, like a watch's.
- **The detail panel goes places.** A pod's controller and its node are links; a controller's and a service's selector is a link *down* to the pods it selects, and a Deployment says explicitly that this is where its Pod logs live. And when `metrics.k8s.io` is installed, a pod or a node says what it is using — a node as a share of its allocatable.
- **A custom resource gets its own columns.** The CRDs are read once, and each kind's `additionalPrinterColumns` — evaluated here as JSONPath against the objects on screen — sit between NAME and AGE, exactly what `kubectl get -o wide` prints. Argo, cert-manager, Crossplane: their tables arrive with no code written for them.
- **Argo CD gets a native surface.** When `Application.argoproj.io` is discovered, Application, ApplicationSet and AppProject sit in an optional GitOps group. An Application's Overview reads project, destination, sources, revision, sync policy, operation, sync and health directly from its CR; its row mark combines Argo's operation, health and sync words. A Resources tab draws `status.resources` as a stable namespace/kind/name-sorted virtualized list, with Argo's sync and health per row. A managed-resource row links to the generic object view only when Argo's destination server matches the selected kubeconfig context's server (ignoring a trailing slash) and discovery says that kind is served; Argo's destination name alone is never treated as a kubeconfig identity. An idle Application offers Sync through the normal `Cluster::patch` path, RBAC check and named second confirmation; the operation is full-app, uses the configured revision and does not enable prune. An ApplicationSet's Overview reads its generators, template project and destination, Go-template mode, strategy and generated count; its row mark follows Argo's own health/condition priority. An AppProject's Overview reads source repositories and namespaces, destinations, resource allow/deny lists, role names, orphan monitoring, project-scoped cluster policy and sync-window count, but never JWT material. No Argo session or API client is introduced.
- **The built-in operational resources have native summaries.** PersistentVolumes and claims explain their binding in both directions; StorageClasses show provisioning policy without resolving anything outside the object; Ingresses show addresses, routes and TLS references; NetworkPolicies show selected pods and isolation rule counts; HPAs show their scale target, replica envelope and resource-metric progress; and ServiceAccounts, Roles and bindings show reference names and permissions without revealing credentials. Unknown kinds still receive metadata, conditions and YAML.
- **A command runs in a container.** A *Run* tab on a pod: a line through `sh -c`, stdout and stderr back, the exit code, or the apiserver's words when it never ran. Gated by the `pods/exec` review.
- **A port on a pod is a port on `localhost`.** A chip per declared container port; one click forwards it, the same number locally when it is free and any free port otherwise, and the panel says what is open. One WebSocket per local connection, so a stuck one stalls nothing else, and a refusal from the apiserver resets that connection the way a pod being down would.
- **It can act, carefully.** Sync an Argo CD Application, scale, restart, trigger/suspend/resume a CronJob, cordon and uncordon, drain, delete, and apply an edited manifest — each reached from the object it acts on, each taking two gestures with the second naming the object, each greyed out when `SelfSubjectAccessReview` says this login may not. Trigger copies only the CronJob's Job template into a one-off Job through a narrow trait method and reviews `create` on Jobs. Nothing destructive is on a key or in the palette (K6).
- **`⌘K` reaches everything by name** — every kind, every namespace, every context, and the four commands that are none of those — ranked by score, and holding nothing that can destroy anything.
- **Switching context** rebuilds the connection and clears everything the last cluster said. An insecure connection says so in the sidebar.
- **English and Japanese.** Every user-visible string is in `locales/app.yml` in both.
- `KIRIKUMO_DEMO=1` runs the whole window over a scripted cluster with something wrong in it, and no network — including a scripted *watch*, so the demo shows a pod restarting, one arriving and one going away without a cluster anywhere. Its Argo CD Application is OutOfSync and Degraded, every entry in its Resources tab names an object that really exists elsewhere in the sample, its owning ApplicationSet is mid-RollingSync, and the Application's production AppProject exists with restrictive policy.

- **A shell into a container.** A *Shell* tab attaches a tty over the exec WebSocket and draws it through `alacritty_terminal` — the emulator Ginka uses, at the same version, so the two are one when they meet (K6 is honoured: attaching is a button on the pod, and a shell only does what is typed into it).

Nothing in M0–M5 is outstanding. The mount into Ginka's window is deferred; the app stands alone.

| # | Name | What lands | Owes |
| --- | --- | --- | --- |
| **M0** | The window | Workspace, tokens, locales, the three-column shell with no title bar, kubeconfig parsing, context list, `/version` handshake, `Scripted` and `KIRIKUMO_DEMO=1` | **Landed.** Visual sign-off against `docs/ui.md` |
| **M1** | Everything is a table | API discovery, the resource tree in the sidebar, one virtualized table for every kind with `kubectl`'s columns — and a custom resource's own, from its CRD — the namespace picker, health marks, the detail's Overview, Events, YAML and Logs | **Landed.** |
| **M2** | Live | Watches wired to the store with bookmarks and re-list, rows updated in place rather than rebuilt, `⌘F`/`⌘L`/`⌘K` | **Landed.** Backoff tuning against a real flaky apiserver rather than a scripted one |
| **M3** | Pods in depth | Logs following, with find and the previous instance; `metrics.k8s.io` for nodes and pods; owner/child navigation | **Landed.** Wrapping long log lines remains (it needs a variable-height virtualized list). Metrics also appear in Pod and Node tables after the optional API first answers, and refresh on their own clock. |
| **M4** | Acting | Delete, scale, restart, CronJob trigger/suspend/resume, cordon/uncordon/drain, apply an edited YAML — each behind a confirmation that names the object, each greyed out when `SelfSubjectAccessReview` says no (K6) | **Landed.** |
| **M5** | Reaching in | Port-forward over WebSocket, one tunnel per local connection, from a chip on the pod; a command run in a container with its output and exit code on a *Run* tab; an interactive shell on a *Shell* tab, drawn by `alacritty_terminal` — **all three landed and verified against a `kind` cluster** (`tests/live.rs`); the mount into Ginka's window, **deferred** — the app stands alone for now (§8, 2026-09-12) | **Landed.** |

### 5.1 Lens parity backlog after v1

This is the explicit boundary between useful parity and copying Lens wholesale. The comparison uses Lens's documented resource views; for example, its [CronJob actions](https://docs.k8slens.dev/k8slens/using-lens/workloads/cron-jobs/) and [configurable ReplicaSet table](https://docs.k8slens.dev/k8slens/using-lens/workloads/replica-sets/).

| Priority | Lens capability not yet in Kirikumo | Direction |
| --- | --- | --- |
| **Now** | Trigger, suspend or resume a CronJob | **Implemented.** Trigger creates one Job from `spec.jobTemplate` through the narrow `Cluster::trigger_cron_job` operation and reviews `create` on Jobs; suspend/resume patch only `spec.suspend`. All use the named second confirmation. |
| **Now** | Resize, reorder and hide table columns | **Implemented.** A table-local column panel changes visibility, order and fixed width per resource, can restore automatic defaults, and persists only headings and preferences — never cells or row data. New discovered columns remain visible by default and NAME cannot be hidden. |
| **Now** | Rich native details for storage, networking, autoscaling and RBAC resources | **Implemented.** Generic JSON-to-view-model functions cover PVs, PVCs, StorageClasses, Ingresses, NetworkPolicies, HPAs, ServiceAccounts, Roles, ClusterRoles and both binding kinds; YAML and the unknown-kind fallback remain intact. |
| **Now** | Export a table | **Implemented as clipboard export.** One control copies the visible headings and filtered, sorted rows as spreadsheet-safe TSV. It follows column order and visibility and writes no cluster payload to disk; Lens-style CSV file export remains out under rule 10. |
| **Out** | Helm/chart management, metrics history/Prometheus, multi-cluster aggregation, node/desktop shell and extension marketplace | Remain explicit §3.2 non-goals. They require a package manager, retained telemetry, aggregation, host terminal or plugin platform rather than a stronger viewer. |

## 6. Quality bars

- **A table of 10 000 rows scrolls at 60 fps.** Every list is one `uniform_list`, and every cell is a string formatted when the object landed, never during a scroll.
- **A watch reconnect is invisible.** A dropped connection must not blank a table or renumber it.
- **No request blocks the window.** Everything through the trait runs on the background executor; a cluster that has gone away shows the last answer and an error line, never a frozen frame.
- **a11y is a rule, not a polish pass.** Every control reachable by mouse is reachable by keyboard with visible focus; health is an icon *and* a colour, never a colour alone.
- **A destructive action takes two deliberate gestures**, and the second one names the object.

## 7. Open questions

| # | Question | Status |
| --- | --- | --- |
| **Q1** | Does the sidebar's resource tree get folded into Ginka's sidebar, or does the cluster surface carry its own tree in the right panel? | Open. The tree is deep enough that Ginka's sidebar may not want it; decide before M5, not during it. |
| **Q2** | Exec and port-forward: WebSocket with `tungstenite`, or `kube-rs` — and with it tokio? | **Resolved for port-forward: `tungstenite`**, over a `rustls` configuration built from the kubeconfig (`kirikumo_kube::tls`). The channel protocol is a byte and two bytes of port; a hundred lines against the scripted cluster and a local echo. `kube-rs` stays out, K3 stays whole, and the question is asked once more only if exec (Q6) turns out to need SPDY. |
| **Q6** | An *interactive* terminal into a container needs a terminal emulator. Ginka has one (`alacritty_terminal`); does this app grow its own, or wait for the mount? | **Resolved: it has one, on the same emulator.** With the mount deferred, waiting meant not having it. `kirikumo_ui::terminal` wraps `alacritty_terminal` at Ginka's version into a `Screen` a view draws as styled runs, and the store pumps one attached tty on a thread of its own — the same shape as a followed log. What Ginka owns is the *dock*; the emulator is a crate, and two wrappers of the same crate fold into one on the day of the mount (§8, 2026-09-13). |
| **Q5** | Do the table's columns come from the apiserver instead of from a list in the source? | **Resolved, the other way round.** The columns a custom resource wants are declared in its CRD as JSONPath (`additionalPrinterColumns`), and the apiserver's `Table` form is only the server evaluating them. Evaluating them *here*, against the objects the window already holds, gives every custom kind the table its authors designed with no second copy of any list, and leaves the watch, the row reuse and the health mark exactly as they were. A subset of JSONPath is enough — the one filter form every CRD uses, `[?(@.type=="Ready")]`, and not much else — and it is verified live against a CRD on `kind`. Built-in kinds keep the hand-written sets, which are `kubectl`'s. |
| **Q3** | Licence | Open, as in Ginka. Nothing copied in, so nothing is settled by accident. |
| **Q4** | Does a shared `glass-tokens` crate get extracted for the three apps, or does each keep its own copy of `Tokens`? | Extract at unification, not before: three copies of a 400-line file that must stay identical is a smell, but a shared crate before there is a host is speculative. |

## 8. Decision log

| Date | Decision | Why |
| --- | --- | --- |
| 2026-09-07 | Kirikumo is a third window in the Ginka family, and every interface decision is made for the embedded case too (§4.3, K1–K6) | The three become one application. A viewer designed only to stand alone would have to be rewritten to be mounted; the constraints cost nothing now and everything later. |
| 2026-09-07 | **`kube-rs` is not used.** The apiserver is reached with `ureq` and a hand-written discovery/watch layer | `kube` is built on `tokio` through `hyper`, and K3 forbids a second reactor in the process Ginka will host. The parts of `kube` we would use — discovery, unstructured lists, watch framing — are a few hundred lines each against a stable, documented API, and writing them keeps the graph free of a runtime. The cost is real: `kube` handles conformance details we will meet one at a time. |
| 2026-09-07 | Objects are `serde_json::Value` plus a parsed `ObjectMeta`, never generated types | A typed model means a code change per CRD, and CRDs are most of what makes a cluster interesting. `metadata` is the one schema every kind shares, so it is the one thing worth typing. |
| 2026-09-07 | The resource catalogue comes from discovery, not from a list in the source | Same reason. A cluster that serves `argoproj.io/Rollout` gets a table without a release here. |
| 2026-09-07 | No disk cache; nothing about a cluster is written to disk | Kubernetes payloads are large, short-lived and frequently secret, and the apiserver does not do `ETag`s. e1 caches to save GitHub's rate limit, which has no analogue here. Settings are the only thing this app writes. |
| 2026-09-07 | Read-only by default, and every write takes two gestures (K6) | The other two apps in the family act on a working copy. This one acts on production, and it will one day share a window — and a key map — with them. |
| 2026-09-07 | `assets/themes/*.json` is Ginka's file byte for byte, not a Kubernetes-flavoured palette | K5. A surface that brings its own colours is a surface that looks bolted on. |
| 2026-09-07 | **K3 is relaxed**: a second async runtime is allowed when it is recorded here and sealed behind the `Cluster` implementation | The original wording — no second reactor, ever — was stronger than Ginka's own rule, which asks for a decision-log entry rather than abstinence. The real constraint is not "one runtime per process" but "a future must be polled by the runtime it was built for", and that is a property of an implementation, not of a workspace. Writing it as a ban would have made a reasonable future choice look like a violation. |
| 2026-09-07 | **`kube-rs` is revisited at M5, and not before** | The value of adopting it is very unevenly distributed. For the read path it is near zero *now*: discovery, unstructured lists, watch framing and the kubeconfig layer are written, tested and about 1,900 lines, and swapping them would not change a pixel. For exec and port-forward it is very high, and that is M5 (Q2). So the read path stays as it is, the question is asked once, at the point where the answer matters, and `Cluster` keeps the swap contained to one implementation if the answer is yes. |
| 2026-09-07 | One watch at a time, for the list on screen, and its retry policy lives on the reading thread | Only one table is showing, so following anything else is streaming a cluster nobody is looking at (§4.7). Putting the reconnect on the thread keeps the store's part down to the one thing a connection cannot decide — that a `410 Gone` means list again — and the thread is told to stop by a flag it can only notice at its next event, which is why the apiserver's watch timeout was cut from half an hour to five minutes. |
| 2026-09-07 | A row is reused when its object's `resourceVersion` has not moved | A watch event changes one object. Rebuilding four thousand rows for it — seven formatted cells each, tens of times a second on a busy namespace — is how a live table becomes a space heater. The version is the cheapest possible proof that nothing changed. |
| 2026-09-07 | Going *down* from a controller is the filter box, not a query | "Which pods does this select?" has no apiserver endpoint — a label selector is a list parameter, not a resource — and building one into the table would be a second query language beside the filter that is already there. Putting the controller's first selector label into the filter box answers the question the way a person would, and leaves them holding the thing that narrowed the table so they can clear it. The cost is that a multi-label selector becomes its first label, which in practice narrows a namespace just as far. |
| 2026-09-07 | The log's find box is literal, not fuzzy — the only box in the window that is | Fuzzy matching over a hundred thousand lines finds every line containing those letters in that order, which is every line. What people do to a log is `grep`, and a filter with a `matched/total` count beside it is that. Highlighting and stepping through matches was the alternative; it needs styled runs inside a line and scroll targeting, and answers a question — "where is the next one?" — that filtering makes not arise. |
| 2026-09-07 | No wrap toggle for logs until the list can have rows of different heights | A wrapped line is a taller row, and the whole reason a fifty-thousand-line scrollback is affordable is that every row is the same height (rule 7). Wrapping by dropping virtualization for the wrapped case would put four thousand elements on screen, which is the thing rule 7 exists to prevent. So: a long line scrolls sideways, and wrapping waits for a variable-height list. |
| 2026-09-07 | A log's scrollback is bounded at 50 000 lines, oldest first | A pod that has been logging for a week would otherwise be held whole in a window nobody closed. Oldest first because the reason anyone follows a log is what happens next. The same shape as a terminal's scrollback in Ginka. |
| 2026-09-12 | The second gesture is a button that names the object, not a field the name is typed into | Typing the name is `kubectl delete`'s weight, and it turns the one action a person makes under pressure into a spelling test. What the safeguard has to achieve is that the second press cannot be made without reading which object it is for; the name on the button, in mono, achieves that. K6 is satisfied by the gesture count and the naming, not by friction. |
| 2026-09-12 | A button this login may not press is grey with a reason, not hidden; and *unknown* is drawn as grey too | Hidden leaves the reader wondering whether the thing can be done at all. Grey with *Not allowed for this login* says exactly why not. And while the `SelfSubjectAccessReview` is in flight the button stays grey, because a control that is pressable for a second and then is not is worse than one that lights up. A cluster that cannot answer the review at all is taken as a yes and the write itself is left to be the judge. |
| 2026-09-12 | The YAML tab is the toolkit's editor, and `tree-sitter-yaml` is the one language feature switched on | Rule: check the gallery before writing a widget. The editor gives line numbers, highlighting, undo and selection for free, and replaces the hand-rolled virtualized list of lines. The feature adds three `tree-sitter` crates and a C build; it is additive, so it costs Ginka nothing until these views are mounted, and then only YAML. |
| 2026-09-12 | *Apply* lives on the YAML tab, beside the text; every other write lives in the footer | The thing being applied is on screen in exactly one place, and the button belongs next to it. Putting *Apply* in the footer would mean a reader on the Overview tab could apply an edit they cannot see. |
| 2026-09-12 | The scripted cluster accepts writes, with a JSON merge patch of its own | So that a demo delete or scale is real and the two gestures can be watched to do something, with no cluster to break. Its strategic merge is a plain merge, which is right for every patch this app sends (none touches a list) and wrong in general; the fake says so in its rustdoc rather than pretending. |
| 2026-09-12 | Drain is its own module, and its policy is `kubectl drain`'s without `--force` | It is the one write that is a *loop with policy in it*: cordon, list the node's pods, skip what a DaemonSet or the kubelet owns, evict each through the Eviction API, and wait out a disruption budget that says no. Kept in `kirikumo_kube::drain` where the scripted cluster can be drained under test. Two departures from `kubectl` are deliberate: a pod nothing controls is *skipped and named* rather than aborting the whole drain, because a button has no `--force` and the report is the honest answer; and a budget that stays full after five tries is reported, never forced. |
| 2026-09-12 | **The mount into Ginka is deferred; Kirikumo stands alone for now** | Said by the user. K1–K6 stay as they are — they cost nothing to keep and everything to recover — but no work is scheduled on the host side, and nothing here waits on it. The one consequence taken now: with no host runtime to answer to, Q2 was decided on the merits of the standalone app. |
| 2026-09-12 | Port-forward is `tungstenite` over a `rustls` configuration of our own, and one WebSocket per local connection | The channel protocol is small — a byte of channel, and on each channel's first message two bytes of port — and writing it kept `kube-rs` and tokio out of a graph that has done without them so far (K3). One WebSocket per connection because the protocol has no way to open a second stream on a connection already up; it is also what keeps one stuck connection from stalling the rest. The pump takes turns on one thread with a short read timeout rather than splitting the socket across two, because a TLS stream cannot be cloned and a `Mutex` around it would block writes behind reads. |
| 2026-09-12 | A forward listens on loopback only | A forwarded port is a hole into a cluster, and the machine's other interfaces are not who asked for it. |
| 2026-09-13 | A live suite against a real apiserver, ignored by default (`crates/kirikumo-kube/tests/live.rs`) | The scripted cluster is right for every change, and wrong for the one question it cannot answer: whether the wire is what we think it is. The first run against `kind` found two things the fake could not — the log subresource refuses `Accept: text/plain` with a 406, and a pod evicted a second ago has nothing listening yet — and both are now in the suite. It writes to the cluster, so it is never run by accident: `--ignored`, one thread, and the drain named to sort last. |
| 2026-09-13 | Exec is non-interactive first: a command in, its output and exit code out, no tty | It is most of what a viewer needs from exec — what is in that file, what does `env` say, is the process there — and it needs no terminal emulator, which is the whole cost of the interactive kind. The wire is the same WebSocket as a port-forward with three more channels, and it is verified live. A command is the reader's own typed words, which is the deliberate act K6 asks for; the `pods/exec` review gates it like a write. stdout and stderr are shown one after the other rather than interleaved, because they are separate channels and the apiserver does not order them against each other — interleaving them here would be inventing an order. |
| 2026-09-13 | A custom resource's columns are its CRD's `additionalPrinterColumns`, evaluated here as JSONPath | Q5 asked whether to let the apiserver print the table. The apiserver only evaluates the same JSONPath the CRD declares, so evaluating it here costs one list of CRDs per connection and keeps every object-shaped thing — the watch, the reused rows, the health mark — as it was. The evaluator is a subset: dotted and bracketed keys, indexes, `[*]`, and the `[?(@.key=="value")]` filter, which is the one form real CRDs actually write. Anything outside it is an empty cell, which is also what `kubectl` shows for a path it cannot follow. Verified against a CRD on `kind`: the cells match `kubectl get -o wide` character for character. |
| 2026-09-13 | Argo CD is a native, optional GitOps surface, not an embedded Web UI or a separate app | The useful state is already present in the Argo CRs, and a manual sync can be requested by setting an Application's top-level operation, so Kirikumo can use the existing `Cluster` boundary and kubeconfig RBAC with no Argo token, cookie, port-forward or WebView. Application, ApplicationSet and AppProject get an optional GitOps sidebar group; Argo Rollouts stays a generic custom resource despite sharing the API group. Managed resources live in their own stable, virtualized list rather than expanding the non-virtualized Overview, and link only when `spec.destination.server` matches the selected context's server and discovery serves the kind. An Argo cluster alias is not a kubeconfig identity. Sync is offered only while no operation is active, uses a merge patch containing `operation.sync`, deliberately omits revision, resource filters and prune, and takes the same RBAC check plus two gestures as every write. ApplicationSet health follows Argo's published priority, with the persisted `status.health` preferred and conditions as the compatibility fallback. AppProject summaries retain policy and role names but deliberately discard both legacy role JWT entries and `status.jwtTokensByRole`. |
| 2026-09-13 | The interactive shell is drawn here, on `alacritty_terminal` at Ginka's version, rather than waiting for the mount | The mount is deferred and a viewer without a shell into a container sends its reader to a terminal for the one thing Lens is opened for most. The emulator is a dependency, not a surface: `kirikumo_ui::terminal::Screen` is a thin wrapper that yields rows of styled spans, and the same wrapper exists in Ginka over the same crate at the same version, so unifying them is a move, not a rewrite (K6 in §4.3). The tunnel lives on one thread that owns it outright — keystrokes and resizes reach it over a channel, output comes back over another — so no lock sits between a keystroke and the wire. The panel measures itself in cells every frame and tells the shell only when the number changes. One shell at a time, dropped when the panel moves to another object: a shell nobody can see is a thread and a socket for nothing. |
| 2026-09-14 | Triggering a CronJob is a narrow `Cluster::trigger_cron_job` operation, not a generic create method | Kirikumo needs Lens's immediate-run workflow, not a second manifest authoring surface. The implementation follows `kubectl create job --from=cronjob`: copy only Job template labels, annotations and spec; force the manual-instantiation annotation; retain the CronJob as controller owner; and let the apiserver generate a collision-resistant name. Its RBAC review targets `create` on Jobs, which is deliberately different from every other footer action's resource. The narrow trait method keeps that multi-resource rule intact when Ginka later proxies it through its daemon. |
| 2026-09-14 | Table column overrides are keyed by resource heading and store presentation only | API discovery can add or remove CRD printer columns between launches, so an index is not a stable identity. Persisting the heading, hidden flag, order and optional pixel width lets the current default set reconcile safely: stale entries disappear, new columns append visibly, and NAME remains present as the row identity. No cell or object crosses the settings boundary, preserving rule 10. |
| 2026-09-14 | Native built-in details are schema adapters over unstructured JSON, not typed resource models | Storage, networking, autoscaling and RBAC objects benefit from a concise operational summary, but introducing generated Kubernetes types would make that presentation path different from every CRD. Small pure functions keep `Object` generic, keep the unknown-kind fallback and YAML available, and make every interpretation testable without GPUI. ServiceAccounts expose only referenced secret names, never secret values. |
| 2026-09-14 | Table export is a clipboard snapshot of the visible grid, never a file | Rule 10 excludes writing cluster payloads to disk, while copying the rows a reader can already see preserves the useful part of Lens export. TSV pastes directly into spreadsheets, includes headings, follows active sort/filter/column preferences, and quotes tabs, line breaks and quotes so cells cannot change the grid. |
| 2026-09-14 | Pod and Node tables opt into CPU and memory columns only after `metrics.k8s.io` succeeds | An absent optional API should not spend permanent width on two columns that can never answer. The first detail/table request discovers support; a successful scope adds CPU and MEMORY before AGE and samples every 30 seconds while that exact table remains visible. Metric values join by namespace and name, participate in row-reuse identity, and sort by their raw quantities rather than formatted text. Unsupported scopes are not retried on the clock. |
