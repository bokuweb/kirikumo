//! The first run against a real apiserver, kept as a test.
//!
//! Everything else in this crate is tested against the scripted cluster,
//! which is the right thing for a suite that runs on every change — but the
//! wire is the wire, and the parts that can only be wrong against a real
//! apiserver are the ones this file exercises: discovery, a list, a watch
//! that sees a real change, a log, a port-forward carrying real HTTP, the
//! access review, and the writes, including creating a Job from a CronJob.
//!
//! Ignored by default. Run with a cluster in the kubeconfig that you do not
//! mind writing to — a `kind` cluster is the intended one — and a namespace
//! named by `KIRIKUMO_LIVE_NAMESPACE` (default `shop`) holding a Deployment
//! called `web` on port 80, a pod called `talker` that logs, and a CRD
//! `widgets.example.kirikumo.dev` with printer columns and a `red-one`
//! widget in `Spinning` phase with a `Ready` condition:
//!
//! ```bash
//! cargo test -p kirikumo-kube --test live -- --ignored --test-threads=1
//! ```
//!
//! One thread, because the writes at the end change the same objects the
//! reads at the start look at — and the drain is named to sort last, because
//! it evicts the very pods the port-forward test forwards to.

use kirikumo_kube::actions;
use kirikumo_kube::{
    Cluster, KubeConfig, LogRequest, ResourceKey, Rest, WatchEvent, kubeconfig_paths,
};
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn namespace() -> String {
    std::env::var("KIRIKUMO_LIVE_NAMESPACE").unwrap_or_else(|_| "shop".to_string())
}

fn connect() -> Rest {
    let config = KubeConfig::load(&kubeconfig_paths()).expect("a kubeconfig");
    let context = config.current_context().expect("a current context");
    Rest::connect(config.access(context).expect("the context resolves")).expect("connects")
}

fn resource(cluster: &Rest, group: &str, kind: &str) -> kirikumo_kube::ApiResource {
    cluster
        .catalogue()
        .expect("discovery")
        .get(&ResourceKey::new(group, kind))
        .cloned()
        .unwrap_or_else(|| panic!("the cluster serves no {kind}"))
}

#[test]
#[ignore = "needs a live cluster"]
fn discovery_finds_the_kinds_every_cluster_has() {
    let cluster = connect();
    let version = cluster.version().expect("/version");
    assert!(version.label().starts_with('v'), "{}", version.label());
    let catalogue = cluster.catalogue().expect("discovery");
    for (group, kind) in [
        ("", "Pod"),
        ("", "Node"),
        ("apps", "Deployment"),
        ("", "ConfigMap"),
        ("rbac.authorization.k8s.io", "Role"),
    ] {
        assert!(
            catalogue.has(&ResourceKey::new(group, kind)),
            "{kind} missing"
        );
    }
    // A subresource never becomes a row.
    assert!(!catalogue.resources.iter().any(|r| r.name.contains('/')));
}

#[test]
#[ignore = "needs a live cluster"]
fn a_list_is_scoped_and_carries_a_version_to_resume_from() {
    let cluster = connect();
    let pods = resource(&cluster, "", "Pod");
    let scoped = cluster.list(&pods, Some(&namespace())).expect("list");
    assert!(
        scoped.items.iter().any(|pod| pod.meta.name == "talker"),
        "the talker pod should be there"
    );
    assert!(!scoped.resource_version.is_empty());
    let all = cluster.list(&pods, None).expect("list all");
    assert!(
        all.items.len() > scoped.items.len(),
        "kube-system has pods too"
    );
    assert!(
        cluster
            .namespaces()
            .expect("namespaces")
            .contains(&namespace())
    );
}

#[test]
#[ignore = "needs a live cluster"]
fn a_watch_sees_a_real_change_and_a_bookmark() {
    let cluster = connect();
    let configmaps = resource(&cluster, "", "ConfigMap");
    let namespace = namespace();
    let list = cluster.list(&configmaps, Some(&namespace)).expect("list");
    let mut stream = cluster
        .watch(&configmaps, Some(&namespace), &list.resource_version)
        .expect("watch");

    // Make a change on another thread, after the watch is up.
    let name = format!("kirikumo-live-{}", std::process::id());
    let writer = connect();
    let writer_ns = namespace.clone();
    let writer_name = name.clone();
    let writer_resource = configmaps.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        // Created through a replace of a not-yet-existing object is not a
        // thing; a merge patch on a missing object 404s. So: a real create
        // via `kubectl`, which is what a person would do.
        let status = std::process::Command::new("kubectl")
            .args([
                "-n",
                &writer_ns,
                "create",
                "configmap",
                &writer_name,
                "--from-literal=a=b",
            ])
            .status();
        assert!(status.is_ok_and(|s| s.success()), "kubectl create");
        std::thread::sleep(Duration::from_millis(500));
        writer
            .delete(&writer_resource, Some(&writer_ns), &writer_name)
            .expect("delete");
    });

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut saw_added = false;
    let mut saw_deleted = false;
    while Instant::now() < deadline && !(saw_added && saw_deleted) {
        match stream.next_event() {
            Some(WatchEvent::Added(object)) if object.meta.name == name => saw_added = true,
            Some(WatchEvent::Deleted(object)) if object.meta.name == name => saw_deleted = true,
            Some(WatchEvent::Failed(error)) => panic!("the watch failed: {error}"),
            Some(_) => {}
            None => break,
        }
    }
    assert!(saw_added, "the watch never saw the configmap appear");
    assert!(saw_deleted, "the watch never saw it go");
}

#[test]
#[ignore = "needs a live cluster"]
fn a_log_can_be_read_and_followed() {
    let cluster = connect();
    let request = LogRequest::new(namespace(), "talker");
    let tail = cluster.logs(&request).expect("logs");
    assert!(tail.contains("tick"), "{tail}");

    let mut stream = cluster.follow_logs(&request).expect("follow");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut lines = 0;
    while Instant::now() < deadline && lines < 3 {
        match stream.next_line() {
            Some(Ok(line)) if line.contains("tick") => lines += 1,
            Some(Ok(_)) => {}
            Some(Err(error)) => panic!("{error}"),
            None => break,
        }
    }
    assert!(
        lines >= 3,
        "a followed log should keep arriving; got {lines}"
    );
}

#[test]
#[ignore = "needs a live cluster"]
fn a_port_forward_carries_real_http_to_a_pod() {
    let cluster = std::sync::Arc::new(connect());
    let pods = resource(&cluster, "", "Pod");
    let namespace = namespace();
    // A *ready* web pod, waited for: the drain test evicts them, and a pod
    // that is still coming up has nothing listening on 80 yet.
    let deadline = Instant::now() + Duration::from_secs(60);
    let web = loop {
        let ready = cluster
            .list(&pods, Some(&namespace))
            .expect("list")
            .items
            .into_iter()
            .find(|pod| {
                pod.meta.name.starts_with("web-")
                    && !pod.meta.is_terminating()
                    && kirikumo_kube::health::of("Pod", pod).level == kirikumo_kube::Level::Ok
            });
        match ready {
            Some(pod) => break pod,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_secs(1)),
            None => panic!("no web pod came ready"),
        }
    };

    let forwarding = cluster.clone();
    let pod = web.meta.name.clone();
    let ns = namespace.clone();
    let forwarder =
        kirikumo_kube::Forwarder::serve(0, move || forwarding.port_forward(&ns, &pod, 80))
            .expect("listen");

    let mut client = TcpStream::connect(("127.0.0.1", forwarder.local_port())).expect("connect");
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    client
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut answer = Vec::new();
    let _ = client.read_to_end(&mut answer);
    let answer = String::from_utf8_lossy(&answer);
    assert!(
        answer.starts_with("HTTP/1.1 200") || answer.starts_with("HTTP/1.0 200"),
        "expected nginx's welcome page, got: {answer:.200}; forwarder says: {:?}",
        forwarder.last_error()
    );
    assert!(answer.contains("nginx"), "{answer:.300}");
    assert!(
        forwarder.last_error().is_none(),
        "{:?}",
        forwarder.last_error()
    );
}

#[test]
#[ignore = "needs a live cluster"]
fn a_command_runs_in_a_container_and_reports_its_exit() {
    let cluster = connect();
    let ok = cluster
        .exec(&kirikumo_kube::ExecRequest::shell(
            namespace(),
            "talker",
            "echo hello from $HOSTNAME; echo warn >&2",
        ))
        .expect("exec");
    assert!(ok.stdout.contains("hello from talker"), "{ok:?}");
    assert!(ok.stderr.contains("warn"), "{ok:?}");
    assert_eq!(ok.exit_code, Some(0), "{ok:?}");
    assert!(ok.succeeded());

    let failed = cluster
        .exec(&kirikumo_kube::ExecRequest::shell(
            namespace(),
            "talker",
            "exit 3",
        ))
        .expect("exec");
    assert_eq!(failed.exit_code, Some(3), "{failed:?}");
    assert!(!failed.succeeded());

    // A container that is not there is the apiserver's refusal, not a hang.
    let missing = cluster
        .exec(&kirikumo_kube::ExecRequest::shell(namespace(), "talker", "true").container("nope"));
    match missing {
        Ok(output) => assert!(output.failure.is_some(), "{output:?}"),
        Err(error) => assert!(
            matches!(
                error,
                kirikumo_kube::Error::Api { .. } | kirikumo_kube::Error::NotFound(_)
            ),
            "{error}"
        ),
    }

    // And the review knows the subresource.
    let pods = resource(&cluster, "", "Pod");
    assert!(
        cluster
            .can_i(
                &kirikumo_kube::exec::review_resource(&pods),
                Some(&namespace()),
                "create"
            )
            .expect("review")
    );
}

#[test]
#[ignore = "needs a live cluster"]
fn a_shell_can_be_attached_to_typed_at_and_resized() {
    use kirikumo_kube::portforward::Poll;
    let cluster = connect();
    let mut shell = cluster
        .attach(&kirikumo_kube::ExecRequest::attach(namespace(), "talker"))
        .expect("attach");
    shell.resize(100, 30).expect("resize");
    // A tty echoes what is typed and then answers it.
    shell.send(b"echo mark-$((40+2))\n").expect("stdin");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        match shell.poll().expect("poll") {
            Poll::Data(bytes) => {
                seen.extend(bytes);
                if String::from_utf8_lossy(&seen).contains("mark-42") {
                    break;
                }
            }
            Poll::Nothing => {}
            Poll::Closed => panic!("the shell closed before answering"),
        }
    }
    let screen = String::from_utf8_lossy(&seen);
    assert!(screen.contains("mark-42"), "{screen}");
    // `exit` ends the session: the status frame closes it.
    shell.send(b"exit\n").expect("stdin");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut closed = false;
    while Instant::now() < deadline {
        if matches!(shell.poll().expect("poll"), Poll::Closed) {
            closed = true;
            break;
        }
    }
    assert!(closed, "the session should end when the shell exits");
    shell.close();
}

#[test]
#[ignore = "needs a live cluster"]
fn a_crds_printer_columns_evaluate_to_what_kubectl_prints() {
    // Needs the `widgets.example.kirikumo.dev` CRD and the `red-one` widget
    // the suite's setup applies (see the module doc).
    let cluster = connect();
    let crds = resource(&cluster, "apiextensions.k8s.io", "CustomResourceDefinition");
    let columns = kirikumo_kube::crd::from_list(&cluster.list(&crds, None).expect("crds").items);
    let widget = ResourceKey::new("example.kirikumo.dev", "Widget");
    let declared = columns
        .get(&widget)
        .expect("the widget CRD declares columns");
    let names: Vec<&str> = declared.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Colour", "Replicas", "Phase", "Ready", "Started"]
    );

    let widgets = resource(&cluster, "example.kirikumo.dev", "Widget");
    let red = cluster
        .get(&widgets, Some(&namespace()), "red-one")
        .expect("red-one");
    let cells: Vec<String> = declared
        .iter()
        .map(|column| kirikumo_kube::jsonpath::cell(&red.raw, &column.json_path))
        .collect();
    // What `kubectl -n shop get widgets -o wide` printed for it.
    assert_eq!(&cells[..4], &["red", "3", "Spinning", "True"]);
    assert!(
        cells[4].ends_with('Z'),
        "a date column is the raw timestamp here: {}",
        cells[4]
    );
}

#[test]
#[ignore = "needs a live cluster"]
fn a_port_forward_to_a_missing_pod_is_refused_with_a_status() {
    let cluster = connect();
    match cluster.port_forward(&namespace(), "no-such-pod", 80) {
        Err(kirikumo_kube::Error::NotFound(_)) => {}
        Err(other) => panic!("expected a not-found, got {other}"),
        Ok(_) => panic!("a tunnel to a pod that is not there"),
    }
}

#[test]
#[ignore = "needs a live cluster"]
fn the_access_review_answers_and_a_scale_lands() {
    let cluster = connect();
    let deployments = resource(&cluster, "apps", "Deployment");
    let namespace = namespace();
    assert!(
        cluster
            .can_i(&deployments, Some(&namespace), "patch")
            .expect("review")
    );

    let before = cluster
        .get(&deployments, Some(&namespace), "web")
        .expect("get");
    let scaled = cluster
        .patch(&deployments, Some(&namespace), "web", actions::scale(3))
        .expect("scale");
    assert_eq!(actions::current_replicas(&scaled), 3);
    assert_ne!(scaled.meta.resource_version, before.meta.resource_version);

    // And back, so the cluster is as it was.
    let back = cluster
        .patch(&deployments, Some(&namespace), "web", actions::scale(2))
        .expect("scale back");
    assert_eq!(actions::current_replicas(&back), 2);

    // A restart stamps the template.
    let restarted = cluster
        .patch(
            &deployments,
            Some(&namespace),
            "web",
            actions::restart(chrono::Utc::now()),
        )
        .expect("restart");
    assert!(
        !restarted
            .str_at("spec.template.metadata.annotations.kubectl.kubernetes.io/restartedAt")
            .is_empty()
            || restarted
                .at("spec.template.metadata.annotations")
                .is_some_and(|a| a.get("kubectl.kubernetes.io/restartedAt").is_some())
    );
}

#[test]
#[ignore = "needs a live cluster"]
fn a_cron_job_can_be_triggered_into_a_real_job() {
    let cluster = connect();
    let namespace = namespace();
    let cron_jobs = resource(&cluster, "batch", "CronJob");
    let jobs = resource(&cluster, "batch", "Job");
    assert!(
        cluster
            .can_i(&jobs, Some(&namespace), "create")
            .expect("review")
    );

    let name = format!("kirikumo-trigger-{}", std::process::id());
    let status = std::process::Command::new("kubectl")
        .args([
            "-n",
            &namespace,
            "create",
            "cronjob",
            &name,
            "--image=busybox",
            "--schedule=0 0 1 1 *",
            "--",
            "echo",
            "kirikumo",
        ])
        .status()
        .expect("kubectl");
    assert!(status.success(), "kubectl create cronjob");

    let cron_job = cluster
        .get(&cron_jobs, Some(&namespace), &name)
        .expect("get cronjob");
    let job = cluster
        .trigger_cron_job(&cron_jobs, &cron_job)
        .expect("trigger");
    assert!(job.meta.name.starts_with(&format!("{name}-manual-")));
    assert_eq!(job.str_at("metadata.ownerReferences.0.name"), name);

    cluster
        .delete(&jobs, Some(&namespace), &job.meta.name)
        .expect("delete job");
    cluster
        .delete(&cron_jobs, Some(&namespace), &name)
        .expect("delete cronjob");
}

#[test]
#[ignore = "needs a live cluster"]
fn an_edited_manifest_applies_and_a_delete_deletes() {
    let cluster = connect();
    let configmaps = resource(&cluster, "", "ConfigMap");
    let namespace = namespace();
    let held = cluster
        .get(&configmaps, Some(&namespace), "web-config")
        .expect("get");
    let mut yaml = kirikumo_kube::yaml::to_yaml(&held.raw);
    yaml = yaml.replace("LOG_LEVEL: info", "LOG_LEVEL: debug");
    let patch = actions::apply(&yaml).expect("a manifest");
    let applied = cluster
        .patch(&configmaps, Some(&namespace), "web-config", patch)
        .expect("apply");
    assert_eq!(applied.str_at("data.LOG_LEVEL"), "debug");

    // Put it back.
    let patch = actions::apply(&yaml.replace("LOG_LEVEL: debug", "LOG_LEVEL: info")).unwrap();
    // The version moved, so a replace has to carry the new one.
    let patch = match patch {
        kirikumo_kube::Patch::Replace(mut value) => {
            value["metadata"]["resourceVersion"] = applied.meta.resource_version.clone().into();
            kirikumo_kube::Patch::Replace(value)
        }
        other => other,
    };
    cluster
        .patch(&configmaps, Some(&namespace), "web-config", patch)
        .expect("apply back");

    // Delete something made for the purpose.
    let name = format!("kirikumo-doomed-{}", std::process::id());
    let status = std::process::Command::new("kubectl")
        .args([
            "-n",
            &namespace,
            "create",
            "configmap",
            &name,
            "--from-literal=x=y",
        ])
        .status()
        .expect("kubectl");
    assert!(status.success());
    cluster
        .delete(&configmaps, Some(&namespace), &name)
        .expect("delete");
    assert!(matches!(
        cluster.get(&configmaps, Some(&namespace), &name),
        Err(kirikumo_kube::Error::NotFound(_))
    ));
}

#[test]
#[ignore = "needs a live cluster"]
fn z_drain_cordons_evicts_and_can_be_undone() {
    let cluster = connect();
    let nodes = resource(&cluster, "", "Node");
    let pods = resource(&cluster, "", "Pod");
    let node = cluster
        .list(&nodes, None)
        .expect("nodes")
        .items
        .into_iter()
        .next()
        .expect("a node");
    let name = node.meta.name.clone();

    let report = kirikumo_kube::drain::drain(&cluster, &nodes, &pods, &name, std::thread::sleep)
        .expect("drain");
    // The web pods are ReplicaSet-owned and move; the bare talker is
    // skipped and named; kube-system's DaemonSet pods stay.
    assert!(report.evicted() >= 2, "{}", report.summary());
    assert!(
        report
            .pods
            .iter()
            .any(|(label, outcome)| label.ends_with("/talker")
                && *outcome
                    == kirikumo_kube::drain::Outcome::Skipped(
                        kirikumo_kube::drain::Skip::Unmanaged
                    )),
        "{:?}",
        report.pods
    );
    let cordoned = cluster.get(&nodes, None, &name).expect("get");
    assert!(cordoned.bool_at("spec.unschedulable"));

    // Uncordon, so the evicted pods can come back.
    cluster
        .patch(&nodes, None, &name, actions::schedulable(false))
        .expect("uncordon");
    let restored = cluster.get(&nodes, None, &name).expect("get");
    assert!(!restored.bool_at("spec.unschedulable"));
}
