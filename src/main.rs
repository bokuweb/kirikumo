//! The Kirikumo desktop app.
//!
//! Deliberately thin: it finds where settings live, decides the language,
//! reads the kubeconfig, opens a frameless window and mounts
//! [`kirikumo_views::Shell`]. Everything that draws is in `kirikumo-views`,
//! so a host can mount the same views without this file (`AGENTS.md` rule 1).

rust_i18n::i18n!("locales", fallback = "en");

use anyhow::Result;
use gpui::{
    App, AppContext as _, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_component::Root;
use kirikumo_kube::{Cluster, KubeConfig, ResourceKey, Rest, Scripted, kubeconfig};
use kirikumo_ui::detail::Target;
use kirikumo_ui::settings::{self, AppSettings};
use kirikumo_ui::{Mode, Paths};
use std::sync::Arc;

/// Where the window's data comes from.
///
/// `KIRIKUMO_DEMO=1` runs over a scripted cluster with no network at all.
/// Otherwise the kubeconfig is read and the remembered context — or the one
/// the file says is current — is connected to.
///
/// A failure here does not stop the window opening: it opens on an empty
/// cluster and says what went wrong, because a window that refuses to open
/// cannot tell the reader which cluster is unreachable (`docs/ui.md` §3.5).
fn source(settings: &AppSettings) -> (Arc<dyn Cluster>, Option<KubeConfig>) {
    if std::env::var_os("KIRIKUMO_DEMO").is_some_and(|value| value == "1") {
        tracing::info!("running over a scripted cluster");
        return (Arc::new(Scripted::sample()), None);
    }
    let paths = kubeconfig::kubeconfig_paths();
    tracing::info!(?paths, "reading the kubeconfig");
    let config = match KubeConfig::load(&paths) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "the kubeconfig could not be read");
            return (Arc::new(Scripted::empty()), None);
        }
    };
    let context = settings
        .last_context
        .clone()
        .filter(|name| config.contexts().iter().any(|entry| &entry.name == name))
        .or_else(|| config.current_context().map(str::to_string));
    let Some(context) = context else {
        tracing::warn!("the kubeconfig names no context");
        return (Arc::new(Scripted::empty()), Some(config));
    };
    match config.access(&context).and_then(Rest::connect) {
        Ok(rest) => {
            tracing::info!(%context, "connected");
            (Arc::new(rest), Some(config))
        }
        Err(error) => {
            tracing::error!(%error, %context, "could not connect");
            (Arc::new(Scripted::empty()), Some(config))
        }
    }
}

/// `KIRIKUMO_DEMO_OPEN=Pod/shop/api-7d9f8c-2xk4t`: an object to open as soon
/// as the window is up, as `Kind/namespace/name` with the namespace left
/// empty for a cluster-scoped kind, and a trailing tab fragment such as
/// `#resources`, `#events`, `#yaml` or `#logs`. For screenshots — and it goes
/// through the same path a link in the detail panel does, so it exercises
/// what a reader would.
fn open_at_launch() -> Option<(Target, Option<String>)> {
    let value = std::env::var("KIRIKUMO_DEMO_OPEN").ok()?;
    // A trailing fragment opens the matching detail tab.
    let (value, tab) = match value.split_once('#') {
        Some((value, tab)) => (value.to_string(), Some(tab.to_string())),
        None => (value, None),
    };
    let mut parts = value.splitn(3, '/');
    let key = ResourceKey::parse(parts.next()?)?;
    let namespace = parts.next()?;
    let name = parts.next()?;
    Some((
        Target::Object {
            key,
            namespace: Some(namespace.to_string()).filter(|namespace| !namespace.is_empty()),
            name: name.to_string(),
        },
        tab,
    ))
}

fn main() -> Result<()> {
    let paths = Paths::from_env()?;
    paths.ensure()?;
    let _log_guard = kirikumo_ui::logging::init(&paths)?;
    let app_settings: AppSettings = settings::load(&paths.app_settings());
    let locale = kirikumo_ui::i18n::init(app_settings.locale.as_deref());
    tracing::info!(%locale, "language");
    let (cluster, config) = source(&app_settings);
    let shell_paths = paths.clone();

    let application = gpui_platform::application().with_assets(kirikumo_ui::Assets);

    application.run(move |cx: &mut App| {
        gpui_component::init(cx);
        kirikumo_views::init(cx);
        let mode = Mode::resolve(app_settings.appearance, cx.window_appearance());
        kirikumo_ui::theme::apply(mode, cx);

        cx.spawn(async move |cx| {
            // No title bar of any kind: the columns run to the top of the
            // window and carry their own controls (`docs/ui.md` §3.1). What
            // is left for the platform is the traffic lights, positioned to
            // sit on the same line as those controls.
            let mut options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(13.), px(15.))),
                }),
                // Our own header strips move the window, so AppKit must not
                // also treat them as a system drag region.
                app_owns_titlebar_drag: true,
                ..Default::default()
            };
            options.window_min_size = Some(size(px(880.), px(560.)));
            // The glass surface of docs/ui.md §1: the window is translucent
            // and the desktop behind it is blurred.
            options.window_background = WindowBackgroundAppearance::Blurred;
            options.window_bounds = Some(cx.update(|cx| {
                WindowBounds::Windowed(Bounds::centered(None, size(px(1440.), px(920.)), cx))
            }));

            let open_palette =
                std::env::var_os("KIRIKUMO_DEMO_PALETTE").is_some_and(|value| value == "1");
            let open = open_at_launch();
            cx.open_window(options, |window, cx| {
                let shell = cx.new(|cx| {
                    kirikumo_views::Shell::new(
                        cluster,
                        config,
                        shell_paths,
                        app_settings,
                        window,
                        cx,
                    )
                });
                if let Some((target, tab)) = open {
                    shell.update(cx, |shell, cx| shell.open_at_launch(target, tab, cx));
                }
                if open_palette {
                    shell.update(cx, |shell, cx| shell.open_palette_at_launch(cx));
                }
                cx.new(|cx| Root::new(shell, window, cx))
            })
            .expect("failed to open the main window");

            // Launched from a terminal rather than an app bundle, the window
            // opens behind whatever was frontmost unless we ask for focus.
            cx.update(|cx| cx.activate(true));
        })
        .detach();
    });

    Ok(())
}
