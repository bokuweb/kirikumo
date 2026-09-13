# Kirikumo UI Specification

> Companion to [`roadmap.md`](roadmap.md). The roadmap says *what* we build and when; this document says *what it looks like* and *which components render it*. Where this document is silent, Ginka's `docs/ui.md` applies: the window, the tokens and the header strips are the same by design (roadmap §4.3, K5).
> Last updated: 2026-09-12

## 1. Design direction

The same three-column workstation as Ginka and e1, on the same dark glass:

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│ ●●●  ⬓        │ Pods · default            [all namespaces ▾] ⌕      ⟳  ⬓        │
├───────────────┼───────────────────────────────────────┼─────────────────────────┤
│ kind-dev      │ ● NAME              READY  STATUS  ↑  │ api-7d9f8c              │
│ v1.31.2       │ ● api-7d9f8c-2xk    2/2    Running    │ Pod · default           │
│               │ ● api-7d9f8c-9qd    2/2    Running    │ ● Running · 2/2 ready   │
│ Cluster       │ ▲ web-6b4c5d-lm2    1/2    Pending    │ node-1 · 10.244.1.7     │
│  Nodes     3  │ ✕ jobrunner-xk4     0/1    Error      │ ─────────────────────── │
│  Namespaces 6 │ ● cache-0           1/1    Running    │ [Overview][Events][YAML]│
│  Events       │                                       │        [Logs]           │
│               │                                       │                         │
│ Workloads     │                                       │ Labels  app=api         │
│  Pods      24 │                                       │ Created 3d ago          │
│  Deployments  │                                       │ Owner   ReplicaSet …    │
│  StatefulSets │                                       │ Containers              │
│  ...          │                                       │  api    ghcr.io/…:1.4   │
│               │                                       │  proxy  envoy:1.31      │
│ Config        │                                       │                         │
│ Network       │                                       │                         │
│ ● watching    │                                       │                         │
└───────────────┴───────────────────────────────────────┴─────────────────────────┘
```

The five properties of Ginka's §1 hold — glass, chromeless, density with air, ambient status, short motion — with two additions:

6. **Health is a mark, not a word.** Every row opens with one 8 px mark in the status colours: a filled dot for healthy, a ring for working, a triangle for attention, a cross for failed, a hollow dot for unknown. The word is in the row too (`Running`, `CrashLoopBackOff`), because the mark is the glance and the word is the answer.
7. **A row is a table row, not a card.** This app's centre column is `kubectl get`, and a person reading it is comparing columns down the page. One line per object, monospace for anything that is an identifier, and the columns of `kubectl get -o wide` in `kubectl`'s own order.

## 2. Design tokens

Identical to Ginka's `docs/ui.md` §2, from the same `assets/themes/*.json` — the same file, byte for byte, not a copy that drifts. The mapping that matters here:

| Health | Token | Mark | Examples |
| --- | --- | --- | --- |
| `Ok` | `status.done` | filled dot | Pod `Running` and ready, Deployment at its replicas, Node `Ready` |
| `Working` | `status.working` | ring | `Pending`, `ContainerCreating`, a rollout in progress, `Terminating` |
| `Attention` | `status.attention` | triangle | ready < desired, a Node under pressure, an unbound PVC |
| `Error` | `status.error` | cross | `CrashLoopBackOff`, `ImagePullBackOff`, `Failed`, `NotReady` |
| `Unknown` | `text.muted` | hollow dot | no status to read, or a kind we have no rule for |

Geometry as Ginka's: 44 px header strips, 4 px grid, row radius 9, controls at 6, sidebar 250 (200–480), right panel 420 (280–…), centre never under 320 px, dividers a 9 px grab area with a hairline in the accent under the pointer, panels sliding over 260 ms with an ease-out cubic.

A table row is **28 px**, not 56: two lines per object is a reading list's shape, and this is a comparison list's. The header row is 26 px on `table_head()`, sticky. Its separator is mode-aware: the dark glass keeps the stronger black band, while the light glass uses only a subtle tint of `text.primary`, so headings retain contrast without turning the table into a grey block.

### Type

The system UI font for labels and prose; **the mono family for every cell that holds an identifier** — names, images, IPs, versions, quantities — at 12 px, because a column of names that do not line up is a column that cannot be scanned. Sidebar rows and headers 13 px, metadata 11.5 px, the detail body 14 px on a 1.6 line height. Logs and YAML are mono at 12 px on a 1.45 line height.

## 3. Regions

### 3.1 Headers — there is no title bar

As Ginka §3.1: each column paints itself to the top and carries a 44 px strip; the leading strip leaves 78 px for the traffic lights; every strip drags the window and double-clicks to zoom. The centre strip says what the table is, and carries the namespace picker, the filter box, refresh and the right-panel toggle.

### 3.2 Sidebar — the cluster and its resources

- **Header** — the context's name in bold and the server's version under it, muted. Clicking it opens the **context picker** as a dropdown anchored directly below the header, without pushing the resource tree down: a filter field and one row per context in the kubeconfig, with the cluster's server address muted beneath each and a check on the current one. Switching drops every watch, clears the store and re-runs discovery; the window does not restart.
- **Groups** — the resource tree, from discovery (roadmap §4.4), in Lens's order because it is the order people already know: **Cluster** (Nodes, Namespaces, Events, and anything else non-namespaced that is not in another group), **Workloads** (Pods, Deployments, DaemonSets, StatefulSets, ReplicaSets, Jobs, CronJobs), **Config** (ConfigMaps, Secrets, ResourceQuotas, LimitRanges, HPAs, PodDisruptionBudgets), **Network** (Services, Endpoints, Ingresses, IngressClasses, NetworkPolicies), **Storage** (PersistentVolumeClaims, PersistentVolumes, StorageClasses), **Access Control** (ServiceAccounts, Roles, RoleBindings, ClusterRoles, ClusterRoleBindings), optional **GitOps** (Argo CD Applications, ApplicationSets and AppProjects), then **Custom Resources**, grouped by API group with the group as the folding heading. GitOps is absent when the cluster does not serve those kinds; Argo Rollouts remains under Custom Resources. A group heading folds and remembers that it did, the way e1's owner headings do. A row is the kind's plural name and, once a list has landed, its count.
- **Anything the catalogue has and these groups do not name** falls into Custom Resources under its group, which is what makes a CRD free: no row here is written in the source.
- **Footer** — what the connection is doing, in one line: the error if there is one, else a green dot and *watching* while the list on screen is being followed, else how many kinds the cluster serves. Then the appearance control (moon, sun, or half of each), the way e1's footer carries it.
- **Insecure clusters say so.** When the context has `insecure-skip-tls-verify`, the header carries a small `status.attention` shield with a tooltip naming the server. A viewer that hides that is worse than one that refuses.

### 3.3 Centre — the table

The centre strip carries the kind's name and the namespace it is scoped to, then the **namespace picker** (a chip that opens a filterable list, with *All namespaces* at the top and disabled for cluster-scoped kinds), the **filter box** (240 px, fuzzy over every cell in the row, live as it is typed), refresh, and the right-panel toggle.

A `uniform_list` of 28 px rows under a sticky header:

- The **health mark** in the first 16 px, then the columns for the kind. The column set is `kubectl get`'s, per kind. A custom resource gets the columns its CRD declares, between NAME and AGE, headings upper-cased the way `kubectl` prints them, numeric ones right-aligned and `date` ones as ages; a CRD that declares none — and anything else with no set of its own — gets Name, Namespace (when namespaced), and Age.
- Names in mono; ages as the shortest unit that says it (`3d`, `2h17m`, `45s`), as `kubectl` writes them.
- The header is clickable and sorts; the arrow says which way. The default is the kind's own: Age descending for Pods and Events, name ascending otherwise.
- Selecting a row draws it in `row.active` and opens it on the right; the table never navigates away underneath.
- **Empty, error and first load**: one muted line for empty; the error's own words in `status.error` over a kept table for a failure; a skeleton of pulsing bars in the row tint for a first load. A refresh over a table that already has rows keeps them and spins the refresh glyph — never a blank page.

### 3.4 Right panel — the object

Tabs under the header, as chips: **Overview**, **Events**, **YAML**, for an Argo CD Application **Resources**, and for anything with containers **Logs**, **Run** and **Shell**.

- **Header** — the object's name at 15/500 in mono; under it the kind, the namespace, the health mark and its word.
- **Overview** — the rows every object has (labels, created, the owner as a link that navigates the centre column, the uid muted), then, when `metrics.k8s.io` is installed and the kind has any, **Using**: CPU in milli-cores and memory in binary units, and for a node the share of its allocatable that is. A cluster with no metrics server gets no *Using* section at all rather than a row of zeroes — a pod using no CPU and a cluster nobody can ask look identical as zeroes and mean opposite things. Then what the kind adds: a Pod's node, IPs, QoS class and its containers with images, ports, restarts and state; a Deployment's strategy, replica counts, conditions and an explicit *Pod logs* route to its selector-filtered pods; a Service's type, cluster IP, ports and selector; a Node's capacity, allocatable, conditions and kubelet version; an Argo CD Application's project, sync, health, operation, destination, revision, sync policy, source repositories and a compact managed-resource summary; an ApplicationSet's generators, template project and destination, Go-template mode, progressive-sync strategy, generated Application count and health; an AppProject's description, allowed source repositories and namespaces, destinations, cluster and namespace resource allow/deny lists, role names, orphan monitoring, cluster scope and sync-window count. JWT tokens are never presentation data. A condition table is the shape `kubectl describe` prints, drawn as rows with the status as a mark.
- **Resources** — only on `Application.argoproj.io`: the flat `status.resources` Argo publishes, sorted by namespace, kind and name so reconciliation does not shuffle unrelated rows. Each 36 px virtualized row shows a health mark, name, kind and namespace, then Argo's sync and health words. It is deliberately not presented as a tree: the Application CR contains no ownership edges. A row becomes a link only when `spec.destination.server` matches the selected context's server and discovery contains its kind; the kind's discovered scope decides whether its namespace is carried into navigation. A destination name is only Argo metadata and never sufficient proof that two contexts are the same cluster.
- **Port forward** — on a pod, under its facts: a chip per declared container port, named when the manifest names it (*http 8080*). One click forwards it to the same port on `localhost` when that is free and to any free port otherwise; the row under the chips says `localhost:8080 → 8080 · 2 open`, with *Copy* for the address and *Stop*. A chip that is already forwarded is grey. Not a write in K6's sense — nothing in the cluster changes — but it opens a port on this machine, so it is one deliberate click on a chip that names the port. A pod that declares no ports says so.
- **Events** — the object's own events, newest first: type as a mark, reason in mono, the message, the count and the age. *No events* is one muted line, and is normal. An Event resource is already a record about another object, so its detail omits this tab rather than offering the usually empty and recursive question “events about this Event”. Refresh always asks the apiserver again, including after an earlier empty result.
- **YAML** — the object as the apiserver sends it, in the toolkit's editor with YAML highlighting. The toolkit's light or dark syntax palette follows the selected app appearance; the editor and gutter use `code.bg`, its text and line numbers use the matching text tokens, and an appearance change updates all of them together. It is read-only, with a line under it saying so, until *Edit*. Editing makes the text the reader's — a watch event landing mid-edit does not replace it — and offers *Apply* and *Cancel* in the same strip. *Apply* takes two gestures like every write (the second says *Apply to name*), and a manifest that is not one — not YAML, not a mapping, no name — is refused here, with the reason where the apiserver's would go, before anything is sent.
- **Logs** — on a Pod, the container picker as chips when there is more than one. On a Deployment, StatefulSet, DaemonSet, ReplicaSet, ReplicationController or Job with a pod template and selector, a first row picks from the matching Pods, newest first, and the next row picks that Pod's container. A Node gets the same explicit picker over Pods whose `spec.nodeName` names it, across namespaces, with each chip written as `namespace/name`. The same toolbar follows: *Follow* (on by default), *Previous* (the instance before this one, which is the only place a crash loop's reason survives), and a find box — and under it the lines in a virtualized list. Run and Shell remain Pod-only: a workload or Node owns several replaceable Pods, so there is no honest implicit target for an interactive session.
  - **Find is literal, not fuzzy.** Every other box in this window is a fuzzy matcher; this one is `grep`, because a fuzzy match over a hundred thousand lines finds every line containing those letters in that order, which is every line. It *filters* rather than highlighting and stepping, and says `matched/total` beside itself so a filtered log is never mistaken for a short one.
  - **Following keeps the end in view, and lets go the moment the reader takes hold.** The list moves only when a line arrives; if it was not at its end when one did, the reader scrolled up to read something and *Follow* switches itself off rather than yanking them back. Pressing *Follow* again jumps to the end and follows on.
  - **There is no wrap toggle**, and there will not be one until the list can have rows of different heights: a wrapped line is a taller row, and a virtualized list of uniform rows is what keeps a fifty-thousand-line scrollback cheap (roadmap §6). A long line scrolls sideways with the list.
  - The scrollback is bounded at 50 000 lines, oldest dropped first.
- **Run** — a command in a container, non-interactively: the container picker as chips, a field, and *Run* (⏎ runs too). The line goes through `sh -c` — `sh` because every image with a shell has it and many have no `bash`. What comes back is stdout, then stderr under a *— stderr —* rule in the error colour, then the exit code (`exit 0` muted, anything else in the error colour), or the apiserver's own words when the command never ran. The two streams are separate channels on the wire and the apiserver does not order them against each other, so they are not interleaved here either. Gated like a write by the `pods/exec` review: no permission, no *Run*. A command is the reader's own typed words, which is the deliberate act; there is no second gesture.
- **Shell** — a terminal into a container: the container picker as chips, *Attach*, and then the screen. Attaching opens a tty (`sh`) over the exec WebSocket and draws it through `alacritty_terminal` (roadmap Q6): every row is one line of styled runs, the sixteen ANSI colours resolved from the theme's own palette and a truecolour escape given exactly. The cursor is a block in the text colour while the panel has keyboard focus and a dimmer one while it does not, so a reader can see where their keystrokes would go before they go there. Clicking the screen focuses it; then every key goes to the shell — `⌃C` and `⌃D` included, arrows and editing keys as their escape sequences — and none reaches the window's own bindings, except `⌘` chords, which stay the window's. The panel measures itself in cells and tells the shell its size whenever that changes, so `vi` and `top` lay out to the width they have. *Detach* closes it; so does moving to another object, and `exit` or `⌃D`, after which the screen stays with *The shell exited.* under it until the reader attaches again. The container cannot be switched under a live shell. Gated like *Run* by the `pods/exec` review. One shell at a time: the panel shows one pod.
- **Footer — the actions strip.** The writes the object offers (roadmap M4): *Sync* for an idle Argo CD Application; *Scale* and *Restart* for a controller; *Cordon* or *Uncordon* and *Drain* for a node; and *Delete* for anything the apiserver lets anyone delete — last, and in the error colour, where a hand does not fall on it. Sync means the whole Application at its configured revision with prune off; it is absent while an operation is active. *Drain* is in the error colour too, because it evicts every pod on the node; when it lands, the strip says what became of them (`4 evicted · 1 skipped`), because a drain that quietly left things behind is worse than one that says it did. *Apply* is not here; it lives beside the text it applies.
  - **Every write takes two gestures, and the second names the object.** Pressing *Delete* turns the strip into *Delete api-7d9f8c-2xk4t* and *Cancel*; pressing *Scale* adds a replica field, pre-filled with what the object has, and *Scale api to 2*. The name is on the button, in the same words `kubectl` would take, so the second press cannot be made without reading which object it is for. The reader does not type the name: that is `kubectl delete`'s weight, and it turns the one action a person makes under pressure into a spelling test.
  - **A button this login may not press is grey, not gone**, with a tooltip saying so. The answer comes from `SelfSubjectAccessReview`, asked once per kind, namespace and verb; until it comes the button is grey too, because a control that is pressable for a second and then is not is worse than one that lights up.
  - While the write is in flight the strip says *Working…*; a refusal is the apiserver's own words in `status.error` under the buttons. Nothing here is bound to a key, and nothing here is in the palette.

Empty state: *Pick something to look at* over the glass.

### 3.5 No cluster

What the centre column is when the kubeconfig has no contexts, or the current one cannot be reached: the logo at 56 px, one sentence naming the file we looked in and what went wrong, and the context picker inline so another one can be tried. Never a modal, and never an empty window with a spinner.

### 3.6 The palette — `⌘K`

An overlay 90 px from the top of the window, 560 px wide, on `bg.raised` with a strong border: a field, and under it up to nine 34 px rows before the list scrolls. Everything else in the window is behind a scrim that dismisses on a click, because a palette that can only be closed with the keyboard is a trap for whoever opened it with the mouse.

Each row is an icon, a title, and a muted hint at the trailing edge — the sidebar group for a kind (and its API group, for a custom resource), *Namespace*, a context's server, *Command*. Where the window already is carries the same check the pickers use.

It holds every kind the cluster serves, every namespace and *All namespaces*, every context in the kubeconfig, and four commands: refresh, the two panel toggles, and the appearance. **Nothing in it is destructive**, for the same reason no key chord is (roadmap K6).

It is the one list in the app **ranked by score** rather than left in its own order. A table is read down a column and must not reorder as you type; a palette is read from the top and must, because the whole value of typing three letters is that the thing you meant is the first row. ↑/↓ (and `⌃P`/`⌃N`) wrap, ⏎ chooses, `esc` dismisses. The list is rebuilt on every open, so a kind the cluster stopped serving is never offered.

## 4. Component mapping

| Region | Component |
| --- | --- |
| Window shell | `gpui-component` `Root`, our header strips |
| Columns | ours: three flex children with explicit widths, a 9 px grab area centred on each divider, and the drag tracked at the window root |
| Table | `gpui::uniform_list` with rows from `kirikumo_ui::table::Row`, our own header |
| Pickers (context, namespace) | ours: a filter `Input` over a `uniform_list` in a `bg.raised` popover |
| Palette | ours: the same, as a centred overlay over a click-to-dismiss scrim |
| Filter box, log find, replica count | `gpui-component` `Input` |
| YAML | `gpui-component` `Editor` with `tree-sitter-yaml`, read-only until *Edit* |
| Logs | `gpui::uniform_list` of mono lines |
| Tooltips, icons | `gpui-component` primitives; our SVGs in `assets/icons/` for what the toolkit lacks |

## 5. Interaction rules

- `⌘B` sidebar, `⌘⌥B` right panel, `⌘R` refresh what is on screen, `⌘F` the filter box, `⌘L` the context picker, `⌘K` the palette.
- **No destructive action has a key chord.** Nothing bound to a key may delete, scale or evict (roadmap K6).
- Picking a row opens it on the right and never navigates the centre away.
- **A fact that goes somewhere is drawn in the accent**, and going there fills the filter box with what narrowed the table — the object's name going *up* to a controller or a node, the controller's first selector label going *down* to its pods. The reader can always see why the table narrowed, because what narrowed it is in the box they can clear. A cluster-scoped kind reached from a namespaced one is not scoped by the namespace it was reached from: a pod's node does not live in the pod's namespace.
- Never block: a fetch or a re-list shows the stale table until the fresh one lands.
- A watch's changes animate nothing. A row that changed re-renders in place; a table that reorders on every heartbeat is unreadable.
- Truncate names from the right; truncate images from the *left*, because the tag is the part being compared.

## 6. The logo

`assets/icons/kirikumo.svg`: three horizontal strokes offset like drifting cloud layers, cut by one vertical, at 2.2 on the 24-grid, one colour. It is painted in `Tokens::logo()` — white on the dark theme, navy on the light one — which is not a token because no other part of the window uses it. It sits on the no-cluster screen at 56 px, and nowhere else.
