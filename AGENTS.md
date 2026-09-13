# AGENTS.md

Guidance for AI coding agents (and humans) working in this repository.

## What this project is

**Kirikumo is a native Kubernetes viewer, written in Rust on GPUI, built to stand on its own and to be mounted inside [Ginka](https://github.com/bokuweb/ginka).**

It shows what a person opens Lens for — what is running, what is unhealthy, what did that pod log, what does this object actually look like — over any cluster their kubeconfig can reach, with nothing installed in the cluster and no account anywhere.

It is the third window in a family that becomes one application: **Ginka** (coding agents) is the host, **[e1](https://github.com/bokuweb/e1)** (GitHub) is a surface in it, and Kirikumo is another. Every interface decision here is made for the embedded case as well as the standalone one; the six constraints that follow from that are `docs/roadmap.md` §4.3, as K1–K6.

**Read [`docs/roadmap.md`](docs/roadmap.md) before starting any non-trivial work.** It holds the architecture, the crate layout, the data model, the embedding contract, the milestone plan and the decision log. This file is the short version; the roadmap is authoritative.

**For anything that renders, read [`docs/ui.md`](docs/ui.md) too.** It holds the layout, the design tokens and the region-by-region breakdown.

## Current state

**M0 to M5 have landed; the mount into Ginka is deferred — the app stands alone for now.** Everything that talks to a cluster has been run against a real one (`crates/kirikumo-kube/tests/live.rs`, a `kind` cluster). The window opens frameless over a blurred desktop with three resizable columns; the kubeconfig layer merges `KUBECONFIG` and authenticates by certificate, token, token file or exec plugin; discovery builds the sidebar's tree, custom resources included; one virtualized table draws every kind with `kubectl get`'s columns — a custom resource's from its CRD, evaluated as JSONPath here — a health mark, a namespace picker and a fuzzy filter; and the detail panel has Overview, Events, YAML and Logs. Switching context rebuilds the connection and clears the last cluster's data.

When Argo CD is installed, its Applications, ApplicationSets and AppProjects get an optional **GitOps** sidebar group. An Application has a native summary — project, sources, destination, revision, sync policy, operation, sync, health and managed-resource counts — plus a virtualized Resources tab over the statuses Argo publishes, all read from the CR through the same `Cluster` path; a row links to the generic object view only when the destination server is the selected cluster and discovery serves its kind. An idle Application can be synced through a full, non-pruning `operation.sync` patch behind the usual RBAC check and named second confirmation. An ApplicationSet natively shows its generators, template project and destination, Go-template mode, progressive-sync strategy, generated Application count and Argo-compatible health. An AppProject shows its source, destination, resource and role boundaries without retaining JWT material. No embedded Web UI or Argo-specific login is involved.

**The table is live.** The list on screen — and only that one — is followed: a thread reads the watch, bookmarks keep the resume point moving, a `410 Gone` re-lists, a dropped connection backs off, and only the rows whose objects actually moved are formatted again. `KIRIKUMO_DEMO=1` has a scripted watch, so the whole path can be exercised without a cluster.

`⌘K` reaches every kind, namespace, context and command by name; the detail panel links up to an object's controller and node and down to what a selector selects, with an explicit Pod logs route from a Deployment, reports what a pod or node is using when the cluster has a metrics server, and **follows a container's log** with a previous-instance toggle and a literal find. **It can act**: sync an Argo CD Application, scale, restart, cordon/uncordon/drain, delete, and apply an edited manifest, each behind a second button that names the object and each greyed out when RBAC says no. **A pod's ports forward to `localhost`** from a chip, one WebSocket per local connection, **a command runs in a container** from a *Run* tab, with its output and exit code, and **a shell attaches to a container** on a *Shell* tab, drawn by `alacritty_terminal` at the version Ginka uses. See `docs/roadmap.md` §5.

## Commands

```bash
cargo run                                   # the desktop app, on the current context
cargo run --release
KIRIKUMO_DEMO=1 cargo run                   # the same window over a scripted cluster, no network
KIRIKUMO_DEMO=1 KIRIKUMO_DEMO_PALETTE=1 cargo run   # ...opened on the palette, for screenshots
KIRIKUMO_DEMO=1 KIRIKUMO_DEMO_OPEN=Pod/shop/api-7d9f8c-2xk4t cargo run   # ...opened on one object
KIRIKUMO_DEMO=1 KIRIKUMO_DEMO_OPEN='Pod/shop/api-7d9f8c-2xk4t#logs' cargo run  # ...on its log, following (#events, #yaml, #run, #shell too)
KIRIKUMO_DEMO=1 KIRIKUMO_DEMO_OPEN='Deployment.apps/shop/api#logs' cargo run  # ...on a workload's Pod picker and log
KIRIKUMO_DEMO=1 KIRIKUMO_DEMO_OPEN='Application.argoproj.io/argocd/shop#resources' cargo run  # ...on the Argo CD Resources tab
KIRIKUMO_DEMO=1 KIRIKUMO_DEMO_OPEN='ApplicationSet.argoproj.io/argocd/environments' cargo run  # ...on an ApplicationSet
KIRIKUMO_DEMO=1 KIRIKUMO_DEMO_OPEN='AppProject.argoproj.io/argocd/production' cargo run  # ...on an AppProject
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo test -p kirikumo-kube -p kirikumo-ui  # the fast loop: no GPUI build
cargo test -p kirikumo-kube --test live -- --ignored --test-threads=1   # against the kubeconfig's
                                            # current cluster; writes to it. See tests/live.rs.
```

On a volume without native extended attributes macOS drops `._*` sidecar files next to every file written; `rust-i18n` reads every file in `locales/`, so delete them (`find . -name '._*' -not -path './target/*' -delete`) before a build that fails on `locales/._app.yml`.

The kubeconfig is looked for in this order: `KIRIKUMO_KUBECONFIG`, then `KUBECONFIG` (a `:`-separated list, merged first-wins), then `~/.kube/config`. `KIRIKUMO_HOME` overrides `~/.kirikumo`, which holds settings and logs and nothing else; `KIRIKUMO_LOG` sets the tracing filter.

## Layout

```
kirikumo/
├─ Cargo.toml           # workspace root; the `kirikumo` binary lives here and is deliberately thin
├─ src/main.rs          # opens the window, mounts kirikumo-views::Shell
├─ crates/
│  ├─ kirikumo-kube/    # domain: kubeconfig, auth, discovery, the object model,
│  │                    # health, the `Cluster` trait, the REST client, watches,
│  │                    # and the scripted fake. No GPUI.
│  ├─ kirikumo-ui/      # design tokens, assets, settings, layout, the sidebar
│  │                    # tree, the table's columns -- everything UI-side that is
│  │                    # testable without a window
│  └─ kirikumo-views/   # the GPUI views, as a library a host window can mount
├─ locales/app.yml      # every user-visible string, en and ja side by side
├─ assets/themes/       # design tokens (dark.json, light.json), Ginka's own files
├─ assets/icons/        # app-owned icons, layered over the toolkit's set
└─ docs/
```

## Architectural rules

These are load-bearing. Each one exists so that Ginka can mount these views; violating one creates work that has to be undone at unification.

1. **The views are a library.** Everything that draws lives in `kirikumo-views`, and `src/main.rs` only opens a window and hands it a `Shell`. A view that only exists in the binary is a view Ginka cannot mount.
2. **A cluster is reached through the `Cluster` trait, never directly.** Views hold an `Arc<dyn Cluster>` and nothing else knows about HTTP. Ginka's daemon owns all state in that app, so when embedded the implementation it supplies will proxy through the daemon — which is only possible if no view has a private path to the network.
3. **One reactor by default, and any second one is sealed behind `Cluster`.** HTTP is blocking (`ureq`) and runs on GPUI's background executor; a watch is a blocking read on a thread of its own; a port-forward is a synchronous `tungstenite` WebSocket on a thread per connection; there is no tokio in the graph. Ginka's rule is that a second runtime needs an entry in its decision log, not that it is banned — so adding one is a decision to record in `docs/roadmap.md` §8, and it must live inside a `Cluster` implementation, because a `hyper` future polled from `smol` panics at run time rather than failing to compile. This is why `kube-rs` is not a dependency *today* and why the question is asked again at M5 (roadmap Q2).
4. **One toolkit, at Ginka's rev.** `gpui-component` is the only linked UI library and it owns the `gpui` rev; `Cargo.lock` pins both to what Ginka's and e1's locks pin. Two revs of `gpui` are two unrelated sets of types. Never pin `gpui` directly.
5. **Tokens by name, and the same names as Ginka.** No view hardcodes a colour, radius or duration; `assets/themes/*.json` is Ginka's file unchanged.
6. **Domain logic belongs in `kirikumo-kube` or `kirikumo-ui`, not in `kirikumo-views`.** If it can be tested without a window, it must live where it can be tested without a window. This is also a compiler constraint: `rustc` overflows its stack expanding `#[test]` in a crate that also holds the toolkit's builder chains, so `kirikumo-views` carries no tests at all.
7. **Long lists are virtualized from the first commit.** The table and the log view are each one `uniform_list`, and the YAML tab is the toolkit's editor, which virtualizes for itself; a namespace with four thousand pods must not cost four thousand elements.
8. **Nothing is typed per kind that can be read generically.** An object is JSON plus its `ObjectMeta`; a column set, a health rule and a detail section are functions of that JSON, and a custom resource's columns are its CRD's own JSONPath evaluated against it. This is what makes a CRD free, and it is the reason there is no code generation here.
9. **Read-only by default, and no write without two deliberate gestures — the second naming the object.** No destructive action is reachable from a key chord. This app acts on production and will one day share a key map with two apps that do not.
10. **Nothing about a cluster is written to disk.** No response cache, no snapshot, no credential. Settings are the only thing this app writes.

## UI stack

- **Linked:** [`gpui-component`](https://github.com/longbridge/gpui-component) — resizable panels, virtualized lists, inputs, tooltips, markdown.
- **Reference, not a dependency:** Ginka's `src/` and e1's `crates/e1-views/` for how the frameless three-column window is assembled. Read for the mechanism, rebuilt here against our own types.
- **Read for behaviour, never copied:** Lens for the shape of the resource tree and the object drawer, k9s for what a row is worth, Headlamp for generic-first rendering, `kubectl` for the wire. Take the requirement away from the reading and implement it here.
- Before writing a widget, check `gpui-component`'s gallery for an existing one.

## Conventions

- **Rust edition 2024.** `cargo fmt` and `cargo clippy -D warnings` must pass; CI enforces both.
- **Errors:** `anyhow` at binary boundaries, typed errors (`thiserror`) inside `kirikumo-kube`.
- **Tests:** test-first for anything with a decision in it — kubeconfig merging and its precedence, exec credential expiry, discovery's preferred-version rule, a Pod's health when a container is in `CrashLoopBackOff` but the phase still says `Running`, `kubectl`'s age formatting, watch framing and `410 Gone`. Cluster behaviour is tested against `kirikumo_kube::Scripted`, never against a live apiserver.
- **i18n:** user-visible strings go through `rust-i18n`. `en` and `ja` are both maintained.
- **a11y is a rule, not a polish pass.** Every control reachable by mouse is reachable by keyboard with visible focus; health is an icon *and* a colour, never a colour alone.
- **English in the repository.** Code, comments, docs, commit messages and pull requests are written in English, no matter what language the conversation that produced them was in.
- **Commits and pull requests:** imperative subject, explain *why* in the body. Reference the roadmap milestone when the change advances one.
- **Comments are rustdoc.** Every public item carries a `///` comment; every crate and module root carries a `//!` header saying what lives there and what it owns. Document what a caller must know — invariants, errors, units, the constraint that made the code look the way it does — not what the signature already says.

## Working agreements for agents

- When a change alters architecture, data model, or scope, **update `docs/roadmap.md` in the same change**, including the decision log at the bottom. When it alters layout, tokens or component choices, update `docs/ui.md`.
- Do not silently expand scope. The milestone ordering and the §3.2 non-goals are deliberate.
- When something here diverges from how Ginka or e1 does the same thing, say why in the decision log — divergence is what unification pays for.
- Keep this file and `CLAUDE.md` truthful. If you add commands, add them here once they actually work.
