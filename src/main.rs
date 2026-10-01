use gpui::{
    App, Application, Bounds, WindowBounds, WindowHandle, WindowKind, WindowOptions, prelude::*,
    px, size,
};
use gpui_component::{Root, Theme, ThemeMode, TitleBar};
use mightty::{action::app_menus, pane_container::PaneContainer, settings::SettingsStore};
use std::borrow::Cow;

#[cfg(windows)]
use {
    gpui::Timer,
    mightty::action::{AppAction, DispatchAppAction},
    mightty::application::{
        ActivationRequest,
        windows::{
            DefaultTerminalHandoff, DefaultTerminalServer, GlobalHotKey, InstanceClaim,
            PrimaryInstance, claim_instance, hide_quick_terminal, quick_terminal_has_focus,
            quick_terminal_is_visible, send_activation, show_default_terminal_window,
            show_quick_terminal, toggle_quick_terminal,
        },
    },
    mightty::profile::ProfileId,
    mightty::settings::{QuickTerminalSettings, ReloadOutcome},
    std::{cell::RefCell, ffi::OsString, rc::Rc, time::Duration},
    url::Url,
};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("ctl") {
        std::process::exit(mightty::control::cli(std::env::args().skip(2).collect()));
    }
    #[cfg(windows)]
    let Some(windows_startup) = windows_startup() else {
        return;
    };

    Application::new().run(move |cx: &mut App| {
        load_embedded_fonts(cx);
        gpui_component::init(cx);
        mightty::widget::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.set_menus(app_menus());

        #[cfg(windows)]
        {
            let (control_tx, control_rx) = flume::bounded(32);
            let control_server = mightty::application::windows::ControlServer::start(control_tx)
                .expect("failed to start control endpoint");
            mightty::control::set_instance_id(control_server.descriptor().instance_id.clone());
            let normal_window = open_normal_window(windows_startup.show_normal_window(), cx);
            start_windows_application(
                windows_startup,
                normal_window,
                control_server,
                control_rx,
                cx,
            );
        }
        #[cfg(not(windows))]
        let _normal_window = open_normal_window(true, cx);

        cx.activate(true);
    });
}

struct TerminalWindow {
    #[cfg(windows)]
    id: String,
    handle: WindowHandle<Root>,
    panes: gpui::Entity<PaneContainer>,
}

fn open_normal_window(show: bool, cx: &mut App) -> TerminalWindow {
    let bounds = Bounds::centered(None, size(px(800.), px(600.0)), cx);
    open_terminal_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitleBar::title_bar_options()),
            show,
            ..Default::default()
        },
        true,
        cx,
    )
}

fn open_terminal_window(
    options: WindowOptions,
    show_titlebar: bool,
    cx: &mut App,
) -> TerminalWindow {
    let settings = SettingsStore::open_default();
    let panes = cx.new(|cx| {
        if show_titlebar {
            PaneContainer::new(settings, cx)
        } else {
            PaneContainer::new_without_titlebar(settings, cx)
        }
    });
    let root_panes = panes.clone();
    let handle = cx
        .open_window(options, move |window, cx| {
            cx.new(|cx| Root::new(root_panes, window, cx))
        })
        .expect("failed to open terminal window");
    let _ = handle.update(cx, |_, window, cx| {
        panes.update(cx, |panes, cx| panes.establish_layout(window, cx));
        let metadata_panes = panes.downgrade();
        let revision = std::cell::Cell::new(0u64);
        window.on_frame_prepared(move |window, cx| {
            revision.set(revision.get() + 1);
            metadata_panes.upgrade().unwrap().update(cx, |panes, cx| {
                panes.presentation(revision.get(), window, cx)
            })
        });
    });
    cx.observe(&panes, |_, _| mightty::diagnostics::mark_dirty())
        .detach();
    let bounds_panes = panes.clone();
    let _ = handle.update(cx, |_, window, cx| {
        bounds_panes.update(cx, |_, cx| {
            cx.observe_window_bounds(window, |panes, window, cx| {
                panes.establish_layout(window, cx);
                mightty::diagnostics::mark_dirty();
            })
            .detach();
            cx.observe_window_activation(window, |_, _, _| mightty::diagnostics::mark_dirty())
                .detach();
        })
    });
    TerminalWindow {
        #[cfg(windows)]
        id: panes.read(cx).window_id().to_owned(),
        handle,
        panes,
    }
}

fn load_embedded_fonts(cx: &mut App) {
    let fonts = vec![
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-Regular.ttf"
        )) as &'static [u8]),
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-Bold.ttf"
        )) as &'static [u8]),
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-Italic.ttf"
        )) as &'static [u8]),
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-BoldItalic.ttf"
        )) as &'static [u8]),
    ];

    cx.text_system()
        .add_fonts(fonts)
        .expect("failed to load embedded JetBrainsMono Nerd Font Mono fonts");
}

#[cfg(windows)]
enum WindowsStartup {
    Application {
        primary_instance: PrimaryInstance,
        request: ActivationRequest,
    },
    Embedding,
    Test,
}

#[cfg(windows)]
impl WindowsStartup {
    fn show_normal_window(&self) -> bool {
        match self {
            Self::Application { request, .. } => request_shows_normal_window(request),
            Self::Embedding => false,
            Self::Test => true,
        }
    }
}

#[cfg(windows)]
fn request_shows_normal_window(request: &ActivationRequest) -> bool {
    !matches!(request, ActivationRequest::OpenQuickTerminal { .. })
}

#[cfg(windows)]
fn windows_startup() -> Option<WindowsStartup> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.first().is_some_and(|v| v == "--test-instance") {
        if let [_, option, directory] = arguments.as_slice()
            && option == "--data-dir"
        {
            let path = std::path::PathBuf::from(directory);
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir().ok()?.join(path)
            };
            match mightty::control::set_test_directory(path) {
                Ok(()) => return Some(WindowsStartup::Test),
                Err(error) => eprintln!("Cannot start isolated instance: {error}"),
            }
        } else {
            eprintln!("use --test-instance --data-dir DIRECTORY");
        }
        return None;
    }
    if is_embedding_arguments(&arguments) {
        return Some(WindowsStartup::Embedding);
    }
    let request = match parse_startup_request(arguments) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("Cannot start mightty: {error}");
            return None;
        }
    };
    match claim_instance() {
        Ok(InstanceClaim::Primary(primary_instance)) => Some(WindowsStartup::Application {
            primary_instance,
            request,
        }),
        Ok(InstanceClaim::Secondary) => {
            if let Err(error) = send_activation(&request) {
                eprintln!("Cannot contact the running mightty process: {error}");
            }
            None
        }
        Err(error) => {
            eprintln!("Cannot create the mightty application instance: {error}");
            None
        }
    }
}

#[cfg(windows)]
fn is_embedding_arguments(arguments: &[OsString]) -> bool {
    matches!(arguments, [argument] if argument == "-Embedding" || argument == "/Embedding")
}

#[cfg(windows)]
fn parse_startup_request(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<ActivationRequest, String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => Ok(ActivationRequest::Activate),
        [uri] if uri.to_string_lossy().starts_with("mightty:") => {
            parse_protocol_request(&uri.to_string_lossy())
        }
        [option] if option == "--quick" => {
            Ok(ActivationRequest::OpenQuickTerminal { profile_id: None })
        }
        [option, profile] if option == "--profile" => Ok(ActivationRequest::OpenProfile {
            profile_id: parse_profile_id(profile)?,
        }),
        [quick, profile_option, profile] if quick == "--quick" && profile_option == "--profile" => {
            Ok(ActivationRequest::OpenQuickTerminal {
                profile_id: Some(parse_profile_id(profile)?),
            })
        }
        _ => Err(
            "use no arguments, '--profile PROFILE', '--quick', or '--quick --profile PROFILE'"
                .to_string(),
        ),
    }
}

#[cfg(windows)]
fn parse_protocol_request(value: &str) -> Result<ActivationRequest, String> {
    let uri = Url::parse(value).map_err(|error| format!("invalid mightty link: {error}"))?;
    if uri.scheme() != "mightty"
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.port().is_some()
        || uri.fragment().is_some()
        || !matches!(uri.path(), "" | "/")
    {
        return Err("invalid mightty link".to_string());
    }

    let operation = uri.host_str().unwrap_or("activate");
    let parameters = uri.query_pairs().collect::<Vec<_>>();
    match (operation, parameters.as_slice()) {
        ("activate", []) => Ok(ActivationRequest::Activate),
        ("quick", []) => Ok(ActivationRequest::OpenQuickTerminal { profile_id: None }),
        ("profile", [(name, value)]) if name == "id" => Ok(ActivationRequest::OpenProfile {
            profile_id: ProfileId::new(value.as_ref()).map_err(|error| error.to_string())?,
        }),
        ("quick", [(name, value)]) if name == "profile" => {
            Ok(ActivationRequest::OpenQuickTerminal {
                profile_id: Some(
                    ProfileId::new(value.as_ref()).map_err(|error| error.to_string())?,
                ),
            })
        }
        _ => Err("use mightty://activate, mightty://quick, or a profile link".to_string()),
    }
}

#[cfg(windows)]
fn parse_profile_id(value: &OsString) -> Result<ProfileId, String> {
    let value = value
        .to_str()
        .ok_or_else(|| "profile IDs must use Unicode text".to_string())?;
    ProfileId::new(value).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn start_windows_application(
    startup: WindowsStartup,
    normal_window: TerminalWindow,
    control_server: mightty::application::windows::ControlServer,
    control_rx: flume::Receiver<mightty::control::Dispatch>,
    cx: &mut App,
) {
    let isolated = matches!(startup, WindowsStartup::Test);
    let (activation_tx, activation_rx) = flume::unbounded();
    let (primary_instance, startup_request, embedding) = match startup {
        WindowsStartup::Application {
            mut primary_instance,
            request,
        } => {
            primary_instance
                .start(activation_tx.clone())
                .expect("failed to start the mightty activation server");
            (Some(primary_instance), Some(request), false)
        }
        WindowsStartup::Embedding => (None, None, true),
        WindowsStartup::Test => (None, None, false),
    };
    let (handoff_tx, handoff_rx) = flume::unbounded();
    let default_terminal_server = (!isolated).then(|| {
        DefaultTerminalServer::start(handoff_tx)
            .expect("failed to start the default-terminal COM server")
    });

    let diagnostic_writer =
        mightty::diagnostics::Writer::start(&control_server.descriptor().instance_id);
    let settings = SettingsStore::open_default();
    let quick_settings = if isolated {
        settings.current().app.quick_terminal.clone()
    } else {
        QuickTerminalSettings::default()
    };
    let last_focused_window = normal_window.id.clone();
    let controller = Rc::new(RefCell::new(WindowsApplication {
        normal_window,
        quick_window: None,
        settings,
        quick_settings,
        activation_tx: (!embedding).then_some(activation_tx),
        primary_instance,
        global_hotkey: None,
        default_terminal_server,
        control_server: Some(control_server),
        control_revision: 1,
        diagnostic_writer,
        diagnostic_state: serde_json::Value::Null,
        started_at: mightty::diagnostics::timestamp(),
        started: std::time::Instant::now(),
        last_persistence: std::time::Instant::now() - Duration::from_secs(1),
        persistence_pending: true,
        subscribers: Vec::new(),
        last_focused_window,
        replace_initial_handoff_tab: embedding,
        shutting_down: false,
    }));
    if !isolated {
        controller.borrow_mut().apply_settings(cx);
    }

    let control_controller = Rc::clone(&controller);
    cx.spawn(async move |cx| {
        while let Ok(dispatch) = control_rx.recv_async().await {
            let result = cx.update(|cx| {
                control_controller
                    .borrow_mut()
                    .control_dispatch(&dispatch, cx)
            });
            if let Ok(value) = result {
                if let Some(value) = value {
                    let _ = dispatch.reply.try_send(value);
                }
            } else {
                break;
            }
        }
    })
    .detach();
    if startup_request
        .as_ref()
        .is_some_and(|request| !request_shows_normal_window(request))
    {
        controller.borrow_mut().ensure_quick_window(cx);
    }

    let action_controller = Rc::clone(&controller);
    cx.on_action::<DispatchAppAction>(move |action, cx| {
        if action.action == AppAction::ToggleQuickTerminal {
            let mut controller = action_controller.borrow_mut();
            if controller.quick_settings.enabled {
                controller.dispatch(
                    ActivationRequest::Dispatch {
                        action: action.action.clone(),
                    },
                    cx,
                );
            }
        }
    });

    let request_controller = Rc::clone(&controller);
    cx.spawn(async move |cx| {
        while let Ok(request) = activation_rx.recv_async().await {
            if cx
                .update(|cx| request_controller.borrow_mut().dispatch(request, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();

    let handoff_controller = Rc::clone(&controller);
    cx.spawn(async move |cx| {
        while let Ok(handoff) = handoff_rx.recv_async().await {
            if cx
                .update(|cx| handoff_controller.borrow_mut().accept_handoff(handoff, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();

    let diagnostic_controller = Rc::clone(&controller);
    cx.spawn(async move |cx| {
        loop {
            Timer::after(Duration::from_millis(100)).await;
            if cx
                .update(|cx| {
                    diagnostic_controller
                        .borrow_mut()
                        .publish_diagnostics(cx, false)
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    let poll_controller = Rc::clone(&controller);
    cx.spawn(async move |cx| {
        loop {
            Timer::after(Duration::from_millis(500)).await;
            if cx
                .update(|cx| poll_controller.borrow_mut().poll(cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();

    let shutdown_controller = Rc::clone(&controller);
    cx.on_app_quit(move |cx| {
        let mut controller = shutdown_controller.borrow_mut();
        controller.publish_diagnostics(cx, true);
        controller.shutdown();
        async {}
    })
    .detach();

    if let Some(startup_request) = startup_request {
        controller.borrow_mut().dispatch(startup_request, cx);
    }
    controller.borrow_mut().publish_diagnostics(cx, true);
}

#[cfg(windows)]
struct WindowsApplication {
    normal_window: TerminalWindow,
    quick_window: Option<TerminalWindow>,
    settings: SettingsStore,
    quick_settings: QuickTerminalSettings,
    activation_tx: Option<flume::Sender<ActivationRequest>>,
    primary_instance: Option<PrimaryInstance>,
    global_hotkey: Option<GlobalHotKey>,
    default_terminal_server: Option<DefaultTerminalServer>,
    replace_initial_handoff_tab: bool,
    shutting_down: bool,
    control_server: Option<mightty::application::windows::ControlServer>,
    control_revision: u64,
    diagnostic_writer: mightty::diagnostics::Writer,
    diagnostic_state: serde_json::Value,
    started_at: String,
    started: std::time::Instant,
    last_persistence: std::time::Instant,
    persistence_pending: bool,
    subscribers: Vec<flume::Sender<serde_json::Value>>,
    last_focused_window: String,
}

#[cfg(windows)]
impl WindowsApplication {
    fn publish_diagnostics(&mut self, cx: &mut App, force: bool) {
        use serde_json::json;
        if !mightty::diagnostics::take_dirty() && !force {
            if self.persistence_pending && self.last_persistence.elapsed() >= Duration::from_secs(1)
            {
                self.diagnostic_writer.submit(self.diagnostic_state.clone());
                self.last_persistence = std::time::Instant::now();
                self.persistence_pending = false;
            }
            return;
        }
        let mut windows = Vec::new();
        for (id, handle, panes) in std::iter::once((
            self.normal_window.id.clone(),
            self.normal_window.handle,
            self.normal_window.panes.clone(),
        ))
        .chain(
            self.quick_window
                .as_ref()
                .map(|w| (w.id.clone(), w.handle, w.panes.clone())),
        ) {
            if let Ok(mut state) =
                handle.update(cx, |_, window, cx| panes.read(cx).control_state(window, cx))
            {
                state["window_id"] = json!(id);
                if state["os_focused"] == true {
                    self.last_focused_window = id;
                }
                windows.push(state);
            }
        }
        let logs = mightty::diagnostics::recent();
        let outcomes = mightty::diagnostics::outcomes();
        let changed = self.diagnostic_state["windows"] != json!(windows)
            || self.diagnostic_state["diagnostics"] != json!(logs)
            || self.diagnostic_state["outcomes"] != json!(outcomes);
        if changed || force {
            self.persistence_pending = true;
            self.control_revision += 1;
            self.diagnostic_state = json!({"schema_version":1,"instance_id":mightty::control::instance_id(),"pid":std::process::id(),"started_at":self.started_at,"uptime_ms":self.started.elapsed().as_millis().to_string(),"observed_at":mightty::diagnostics::timestamp(),"revision":self.control_revision.to_string(),"source":"live","orderly_shutdown":false,"last_focused_window_id":self.last_focused_window,"build":{"mightty_version":env!("CARGO_PKG_VERSION"),"mightty_revision":env!("MIGHTTY_BUILD_REVISION"),"target":env!("MIGHTTY_BUILD_TARGET"),"profile":env!("MIGHTTY_BUILD_PROFILE"),"ghostty_optimize":env!("MIGHTTY_GHOSTTY_OPTIMIZE"),"ghostty_revision":mightty::ghostty::SOURCE_REVISION},"windows":windows,"outcomes":outcomes,"diagnostics":logs,"persistence":self.diagnostic_writer.status()});
            mightty::snapshot::publish_state(self.diagnostic_state.clone());
            let event = json!({"protocol_version":1,"instance_id":mightty::control::instance_id(),"revision":self.control_revision.to_string(),"type":"change","changes":[{"kind":"windows_changed","windows":self.diagnostic_state["windows"]},{"kind":"outcomes_changed","outcomes":self.diagnostic_state["outcomes"]},{"kind":"diagnostics_changed","diagnostics":self.diagnostic_state["diagnostics"]}]});
            self.subscribers.retain(|sender|{if sender.len()>=31 {let _=sender.try_send(json!({"protocol_version":1,"instance_id":mightty::control::instance_id(),"revision":self.control_revision.to_string(),"type":"resync_required"}));false}else{sender.try_send(event.clone()).is_ok()}});
        }
        // Debounce traversal/publication separately from disk I/O; continuous output gets a one-second write deadline.
        if self.persistence_pending
            && (force || self.last_persistence.elapsed() >= Duration::from_secs(1))
        {
            self.diagnostic_writer.submit(self.diagnostic_state.clone());
            self.last_persistence = std::time::Instant::now();
            self.persistence_pending = false;
        }
    }
    fn control_dispatch(
        &mut self,
        dispatch: &mightty::control::Dispatch,
        cx: &mut App,
    ) -> Option<serde_json::Value> {
        use mightty::control::{ControlError, reply};
        use serde_json::json;
        let request = &dispatch.request;
        self.publish_diagnostics(cx, false);
        if let Some(outcome) = mightty::diagnostics::outcome(&request.target) {
            if request.op == "state" {
                return Some(reply(request, self.control_revision, Ok(outcome)));
            }
            if request.op == "wait" && request.args["condition"] == "process-exited" {
                let request = request.clone();
                let sender = dispatch.reply.clone();
                let deadline = dispatch.deadline;
                let revision = self.control_revision;
                cx.spawn(async move |_| {
                    loop {
                        let result = match mightty::diagnostics::outcome(&request.target) {
                            Some(outcome)
                                if outcome["processes"]["availability"] == "unavailable" =>
                            {
                                Some(Err(ControlError::new(
                                    "unavailable",
                                    "root process observation unavailable",
                                )))
                            }
                            Some(outcome) if outcome["processes"]["lifecycle"] == "exited" => Some(
                                Ok(json!({"condition":"process-exited","observation":outcome})),
                            ),
                            Some(outcome) if std::time::Instant::now() >= deadline => {
                                let mut error =
                                    ControlError::new("timeout", "root exit was not observed");
                                error.details = Box::new(json!({"last_observation":outcome}));
                                Some(Err(error))
                            }
                            Some(_) => None,
                            None => Some(Err(ControlError::new(
                                "outcome_expired",
                                "removed pane outcome expired",
                            ))),
                        };
                        if let Some(result) = result {
                            let _ = sender.try_send(reply(&request, revision, result));
                            break;
                        }
                        if sender.is_disconnected() {
                            break;
                        }
                        Timer::after(Duration::from_millis(50)).await;
                    }
                })
                .detach();
                return None;
            }
            let mut error = ControlError::new("pane_closed", "pane was removed");
            error.details = Box::new(json!({"outcome":outcome}));
            return Some(reply(request, self.control_revision, Err(error)));
        }
        if std::time::Instant::now() >= dispatch.deadline {
            return Some(reply(
                request,
                self.control_revision,
                Err(ControlError::new(
                    "cancelled",
                    "request expired before dispatch",
                )),
            ));
        }
        let mut windows = vec![(
            self.normal_window.id.clone(),
            self.normal_window.handle,
            self.normal_window.panes.clone(),
        )];
        if let Some(quick) = &self.quick_window {
            windows.push((quick.id.clone(), quick.handle, quick.panes.clone()));
        }
        windows.retain(|(_, handle, _)| handle.is_active(cx).is_some());
        if request.op == "state"
            && request.target.window_id.is_none()
            && request.target.tab_id.is_none()
            && request.target.pane_id.is_none()
        {
            return Some(reply(
                request,
                self.control_revision,
                Ok({
                    let mut state = self.diagnostic_state.clone();
                    state["persistence"] = self.diagnostic_writer.status();
                    state
                }),
            ));
        }
        if request.op == "events" {
            if self.subscribers.len() >= 8 {
                return Some(reply(
                    request,
                    self.control_revision,
                    Err(ControlError::new("busy", "event subscriber limit reached")),
                ));
            }
            let _=dispatch.reply.try_send(json!({"protocol_version":1,"instance_id":mightty::control::instance_id(),"revision":self.control_revision.to_string(),"type":"state","state":self.diagnostic_state}));
            self.subscribers.push(dispatch.reply.clone());
            return None;
        }
        if request.op == "profiles" {
            return Some(reply(request,self.control_revision,Ok(json!(self.settings.current().profiles.iter().map(|(id,profile)|json!({"profile_id":id.as_str(),"label":profile.label,"executable":profile.launch.executable,"argv":profile.launch.arguments.iter().map(|v|v.to_string_lossy()).collect::<Vec<_>>(),"working_directory":profile.launch.working_directory})).collect::<Vec<_>>()))));
        }
        if request.op == "capabilities" {
            return Some(reply(
                request,
                self.control_revision,
                Ok(mightty::control::capabilities()),
            ));
        }
        let active = windows
            .iter()
            .find(|(_, handle, _)| handle.is_active(cx) == Some(true))
            .map(|(id, _, _)| id.clone())
            .unwrap_or_else(|| self.last_focused_window.clone());
        let targets = windows
            .into_iter()
            .filter(|(id, _, panes)| {
                request
                    .target
                    .window_id
                    .as_ref()
                    .is_none_or(|w| w == id || (w == "active" && *id == active))
                    && (request.target.window_id.is_some()
                        || (request.target.tab_id.as_deref() != Some("active")
                            && request.target.pane_id.as_deref() != Some("active"))
                        || *id == active)
                    && panes.read(cx).control_contains(&request.target, cx)
            })
            .collect::<Vec<_>>();
        if request.op == "snapshot"
            && let [(_, handle, panes)] = targets.as_slice()
        {
            let permit = match mightty::snapshot::Permit::acquire() {
                Ok(permit) => permit,
                Err(error) => return Some(reply(request, self.control_revision, Err(error))),
            };
            let resolved = panes.read(cx).resolve_snapshot_target(request, cx);
            let request = match resolved {
                Ok(request) => request,
                Err(error) => return Some(reply(request, self.control_revision, Err(error))),
            };
            let mode = match mightty::control::string_arg(&request, "frame") {
                Ok(mode) => mode.unwrap_or(
                    if request.target.tab_id.is_some() || request.target.pane_id.is_some() {
                        "offscreen"
                    } else {
                        "presented"
                    },
                ),
                Err(error) => return Some(reply(&request, self.control_revision, Err(error))),
            };
            if !matches!(mode, "presented" | "next" | "offscreen") {
                return Some(reply(
                    &request,
                    self.control_revision,
                    Err(ControlError::new(
                        "invalid_argument",
                        "frame must be presented, next or offscreen",
                    )),
                ));
            }
            if mode == "offscreen"
                && request.target.tab_id.is_none()
                && request.target.pane_id.is_none()
            {
                return Some(reply(
                    &request,
                    self.control_revision,
                    Err(ControlError::new(
                        "unsupported",
                        "offscreen requires a tab or pane target",
                    )),
                ));
            }
            let handle = gpui::AnyWindowHandle::from(*handle);
            if mode == "next" {
                let ready = handle
                    .update(cx, |_, window, cx| {
                        let visibility = mightty::snapshot::visibility(window);
                        if visibility["visible"] != true || visibility["minimized"] == true {
                            return Err(ControlError::new(
                                "not_presentable",
                                "window is hidden or minimized",
                            ));
                        }
                        panes.read(cx).snapshot_layout_ready(&request, window, cx)?;
                        Ok(())
                    })
                    .map_err(|e| ControlError::new("window_closed", e.to_string()))
                    .and_then(|r| r);
                if let Err(error) = ready {
                    return Some(reply(&request, self.control_revision, Err(error)));
                }
            }
            let sender = dispatch.reply.clone();
            let deadline = dispatch.deadline;
            let revision = self.control_revision;
            let state = self.diagnostic_state.clone();
            let next = mode == "next";
            let offscreen = mode == "offscreen";
            let mut immediate = if !next && !offscreen {
                Some(
                    handle
                        .update(cx, |_, window, _| {
                            mightty::snapshot::acquire_presented(&request, window)
                        })
                        .map_err(|e| ControlError::new("window_closed", e.to_string()))
                        .and_then(|r| r),
                )
            } else {
                None
            };
            let panes = panes.clone();
            cx.spawn(async move |cx| {
                let mut baseline = 0;
                let mut requested = false;
                let prepared = loop {
                    if let Some(prepared) = immediate.take() {
                        break prepared;
                    }
                    let result = handle.update(cx, |_, window, cx| {
                        match panes.read(cx).snapshot_layout_ready(&request, window, cx) {
                            Err(error) => return Some(Err(error)),
                            Ok(false) => return None,
                            Ok(true) => {}
                        }
                        if offscreen {
                            return Some(
                                panes.update(cx, |panes, cx| panes.offscreen(&request, window, cx)),
                            );
                        }
                        if next {
                            match panes.read(cx).snapshot_presentation_ready(&request, cx) {
                                Err(error) => return Some(Err(error)),
                                Ok(false) => {
                                    requested = false;
                                    return None;
                                }
                                Ok(true) => {}
                            }
                        }
                        if !requested {
                            baseline = window
                                .prepared_metadata()
                                .and_then(|data| data.downcast::<mightty::snapshot::Frame>().ok())
                                .and_then(|frame| frame.scene_revision.parse::<u64>().ok())
                                .unwrap_or(0);
                            window.refresh();
                            requested = true;
                            return None;
                        }
                        let fresh_scene = window
                            .presented_metadata()
                            .and_then(|data| data.downcast::<mightty::snapshot::Frame>().ok())
                            .and_then(|frame| frame.scene_revision.parse::<u64>().ok())
                            .is_some_and(|revision| revision > baseline);
                        if !next || fresh_scene {
                            Some(mightty::snapshot::acquire_presented(&request, window))
                        } else {
                            None
                        }
                    });
                    match result {
                        Ok(Some(result)) => break result,
                        Err(error) => {
                            break Err(ControlError::new("window_closed", error.to_string()));
                        }
                        _ if std::time::Instant::now() >= deadline => {
                            break Err(ControlError::new(
                                if offscreen {
                                    "layout_unavailable"
                                } else {
                                    "timeout"
                                },
                                "layout or requested presentation did not become ready",
                            ));
                        }
                        _ => {
                            Timer::after(Duration::from_millis(10)).await;
                        }
                    }
                };
                let prepared = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _ = sender.try_send(reply(&request, revision, Err(error)));
                        return;
                    }
                };
                let (done, result) = flume::bounded(1);
                let work_request = request.clone();
                cx.background_executor()
                    .spawn(async move {
                        let _ = done.try_send(mightty::snapshot::write(
                            prepared,
                            work_request,
                            state,
                            permit,
                        ));
                    })
                    .detach();
                loop {
                    if let Ok(result) = result.try_recv() {
                        let response = reply(&request, revision, result);
                        mightty::diagnostics::record_control(&request, &response);
                        let _ = sender.try_send(response);
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        let mut error = ControlError::new(
                            "outcome_unknown",
                            "capture worker did not finish within deadline",
                        );
                        error.effect = "unknown";
                        let _ = sender.try_send(reply(&request, revision, Err(error)));
                        break;
                    }
                    Timer::after(Duration::from_millis(10)).await;
                }
            })
            .detach();
            return None;
        }
        if matches!(
            request.op.as_str(),
            "ui.key" | "ui.text" | "ui.pointer" | "ui.input"
        ) && let [(_, handle, panes)] = targets.as_slice()
        {
            let handle = gpui::AnyWindowHandle::from(*handle);
            let registration = handle
                .update(cx, |_, window, cx| {
                    if request.target.tab_id.is_some() || request.target.pane_id.is_some() {
                        return Err(ControlError::new(
                            "invalid_target",
                            "UI injection targets a window",
                        ));
                    }
                    panes.read(cx).check_layout(request, None, window, cx)?;
                    mightty::ui_control::prepare(request, window, cx)
                })
                .map_err(|e| ControlError::new("window_closed", e.to_string()))
                .and_then(|r| r);
            let mut sequence = match registration {
                Ok(sequence) => sequence,
                Err(error) => return Some(reply(request, self.control_revision, Err(error))),
            };
            let panes = panes.clone();
            let request = request.clone();
            let deadline = dispatch.deadline;
            let sender = dispatch.reply.clone();
            let revision = self.control_revision;
            cx.spawn(async move |cx| {
                loop {
                    let cancelled =
                        sender.is_disconnected() || std::time::Instant::now() >= deadline;
                    let progress = handle
                        .update(cx, |_, window, cx| {
                            if cancelled {
                                Err(sequence.cancel(window, cx))
                            } else {
                                sequence
                                    .receiving_targets
                                    .push(panes.read(cx).receiving_focus(window, cx));
                                sequence.next(window, cx)
                            }
                        })
                        .map_err(|e| ControlError::new("window_closed", e.to_string()))
                        .and_then(|r| r);
                    let completion = match progress {
                        Ok(false) => None,
                        Ok(true) => Some(Ok(sequence.result())),
                        Err(error) => Some(Err(error)),
                    };
                    if let Some(result) = completion {
                        let response = reply(&request, revision, result);
                        mightty::diagnostics::record_control(&request, &response);
                        let _ = sender.try_send(response);
                        break;
                    }
                    // Let GPUI activate subscriptions and finish normal input/focus effects between steps.
                    Timer::after(Duration::from_millis(15)).await;
                }
            })
            .detach();
            return None;
        }
        if request.op == "wait"
            && let [(_, handle, panes)] = targets.as_slice()
        {
            let registration =
                panes.update(cx, |panes, cx| panes.register_control_wait(request, cx));
            let mut wait = match registration {
                Ok(wait) => wait,
                Err(error) => return Some(reply(request, self.control_revision, Err(error))),
            };
            let initial = handle.update(cx, |_, window, cx| {
                panes.update(cx, |panes, cx| {
                    panes.probe_control_wait(&mut wait, window, cx)
                })
            });
            match initial {
                Ok(Ok(None)) => {}
                result => {
                    wait.release(cx);
                    return Some(reply(
                        request,
                        self.control_revision,
                        result
                            .map_err(|e| ControlError::new("window_closed", e.to_string()))
                            .and_then(|r| r)
                            .map(|v| v.unwrap()),
                    ));
                }
            }
            let handle = *handle;
            let panes = panes.clone();
            let request = request.clone();
            let sender = dispatch.reply.clone();
            let deadline = dispatch.deadline;
            let revision = self.control_revision;
            cx.spawn(async move |cx| {
                loop {
                    if sender.is_disconnected() {
                        let _ = cx.update(|cx| wait.release(cx));
                        break;
                    }
                    Timer::after(Duration::from_millis(15)).await;
                    let observed = handle.update(cx, |_, window, cx| {
                        panes.update(cx, |panes, cx| {
                            panes.probe_control_wait(&mut wait, window, cx)
                        })
                    });
                    let result = match observed {
                        Ok(result) => result,
                        Err(_) => wait.probe_removed_window(),
                    };
                    let completion = match result {
                        Ok(Some(value)) => Some(Ok(value)),
                        Err(error) => Some(Err(error)),
                        Ok(None) if std::time::Instant::now() >= deadline => {
                            let mut error =
                                ControlError::new("timeout", "wait condition was not observed");
                            error.details = Box::new(json!({"last_observation":wait.last}));
                            Some(Err(error))
                        }
                        _ => None,
                    };
                    if let Some(result) = completion {
                        let _ = cx.update(|cx| wait.release(cx));
                        let _ = sender.try_send(reply(&request, revision, result));
                        break;
                    }
                }
            })
            .detach();
            return None;
        }
        let result = match targets.as_slice() {
            [(id, handle, panes)] => gpui::AnyWindowHandle::from(*handle)
                .update(cx, |_, window, cx| {
                    panes.update(cx, |panes, cx| panes.control_dispatch(request, window, cx))
                })
                .map_err(|e| ControlError::new("window_closed", e.to_string()))
                .and_then(|r| r)
                .map(|mut value| {
                    if value.is_object() {
                        value["window_id"] = json!(id);
                    }
                    value
                }),
            [] => Err(ControlError::new(
                "target_unavailable",
                "stale or conflicting window/tab/pane target",
            )),
            _ => Err(ControlError::new("ambiguous_target", "specify a window")),
        };
        if request.op == "window.resize"
            && result.is_ok()
            && let [(id, handle, panes)] = targets.as_slice()
        {
            let id = (*id).to_string();
            let handle = *handle;
            let panes = panes.clone();
            let request = request.clone();
            let deadline = dispatch.deadline;
            let sender = dispatch.reply.clone();
            let revision = self.control_revision;
            let initial = result.as_ref().unwrap()["bounds"].clone();
            cx.spawn(async move |cx| {
                loop {
                    let observed = handle.update(cx, |_, window, cx| {
                        let width = f32::from(window.viewport_size().width);
                        let height = f32::from(window.viewport_size().height);
                        let requested_width = request.args["width"].as_f64().unwrap() as f32;
                        let requested_height = request.args["height"].as_f64().unwrap() as f32;
                        if (width == requested_width && height == requested_height)
                            || f64::from(width) != initial["width"].as_f64().unwrap()
                            || f64::from(height) != initial["height"].as_f64().unwrap()
                        {
                            Some(panes.update(cx, |panes, cx| {
                                panes.establish_layout(window, cx);
                                let mut state = panes.control_state(window, cx);
                                state["window_id"] = json!(id);
                                state["clamped"] =
                                    json!(width != requested_width || height != requested_height);
                                (state, panes.take_control_acks(cx))
                            }))
                        } else {
                            None
                        }
                    });
                    match observed {
                        Ok(Some((state, acks))) => {
                            let response = mightty::control::complete_acknowledgements(
                                reply(&request, revision, Ok(state)),
                                acks,
                                deadline,
                            )
                            .await;
                            mightty::diagnostics::record_control(&request, &response);
                            let _ = sender.try_send(response);
                            break;
                        }
                        Ok(None) if std::time::Instant::now() < deadline => {
                            Timer::after(Duration::from_millis(5)).await;
                        }
                        _ => {
                            let mut error = ControlError::new(
                                "outcome_unknown",
                                "window resize completion unavailable",
                            );
                            error.effect = "unknown";
                            let _ = sender.try_send(reply(&request, revision, Err(error)));
                            break;
                        }
                    }
                }
            })
            .detach();
            return None;
        }
        let mut acknowledgements = Vec::new();
        if result.is_ok()
            && !matches!(
                request.op.as_str(),
                "state"
                    | "profiles"
                    | "capabilities"
                    | "pane.read"
                    | "ui.key"
                    | "ui.text"
                    | "ui.pointer"
                    | "ui.input"
            )
            && let [(_, _, panes)] = targets.as_slice()
        {
            acknowledgements = panes.update(cx, |panes, cx| panes.take_control_acks(cx));
        }
        self.publish_diagnostics(cx, false);
        let response = reply(request, self.control_revision, result);
        if acknowledgements.is_empty()
            && (!matches!(request.op.as_str(), "state" | "pane.read" | "profiles")
                || response["ok"] != true)
        {
            mightty::diagnostics::record_control(request, &response);
        }
        if acknowledgements.is_empty() {
            return Some(response);
        }
        let deadline = dispatch.deadline;
        let sender = dispatch.reply.clone();
        let request = request.clone();
        cx.spawn(async move |_| {
            let response =
                mightty::control::complete_acknowledgements(response, acknowledgements, deadline)
                    .await;
            mightty::diagnostics::record_control(&request, &response);
            let _ = sender.try_send(response);
        })
        .detach();
        None
    }
    fn dispatch(&mut self, request: ActivationRequest, cx: &mut App) {
        match request {
            ActivationRequest::Activate => self.activate_normal(cx),
            ActivationRequest::OpenProfile { profile_id } => {
                self.activate_normal(cx);
                self.dispatch_to_normal(
                    AppAction::NewTab {
                        profile_id: Some(profile_id),
                    },
                    cx,
                );
            }
            ActivationRequest::OpenQuickTerminal { profile_id } => {
                let window = self.ensure_quick_window(cx);
                let settings = self.quick_settings.clone();
                let _ = window.update(cx, |_, window, _| show_quick_terminal(window, &settings));
                if let Some(profile_id) = profile_id {
                    Self::dispatch_to_window(
                        window,
                        AppAction::NewTab {
                            profile_id: Some(profile_id),
                        },
                        cx,
                    );
                }
            }
            ActivationRequest::Dispatch {
                action: AppAction::ToggleQuickTerminal,
            } => {
                let window = self.ensure_quick_window(cx);
                let settings = self.quick_settings.clone();
                let _ = window.update(cx, |_, window, _| toggle_quick_terminal(window, &settings));
            }
            ActivationRequest::Dispatch { action } => {
                self.dispatch_to_normal(action, cx);
            }
        }
    }

    fn poll(&mut self, cx: &mut App) {
        match self.settings.reload_if_changed() {
            ReloadOutcome::Applied { .. } => self.apply_settings(cx),
            ReloadOutcome::Rejected(diagnostic) => {
                mightty::diagnostics::record(
                    "settings",
                    "reload_rejected",
                    &diagnostic.to_string(),
                    serde_json::json!({"instance_id":mightty::control::instance_id()}),
                );
            }
            ReloadOutcome::Unchanged => {}
        }

        if !self.quick_settings.hide_on_focus_loss {
            return;
        }
        let Some(window) = &self.quick_window else {
            return;
        };
        let _ = window.handle.update(cx, |_, window, _| {
            if quick_terminal_is_visible(window)? && !quick_terminal_has_focus(window)? {
                hide_quick_terminal(window)?;
            }
            std::io::Result::Ok(())
        });
    }

    fn apply_settings(&mut self, cx: &mut App) {
        if mightty::control::test_directory().is_some() {
            return;
        }
        let quick_settings = self.settings.current().app.quick_terminal.clone();
        if quick_settings == self.quick_settings {
            return;
        }

        let hotkey_changed = quick_settings.enabled != self.quick_settings.enabled
            || quick_settings.hotkey != self.quick_settings.hotkey;
        self.quick_settings = quick_settings;
        if hotkey_changed {
            self.global_hotkey = None;
            if self.quick_settings.enabled
                && let Some(sender) = &self.activation_tx
            {
                match GlobalHotKey::register(&self.quick_settings.hotkey, sender.clone()) {
                    Ok(hotkey) => self.global_hotkey = Some(hotkey),
                    Err(error) => eprintln!(
                        "Cannot register quick-terminal hotkey '{}': {error}",
                        self.quick_settings.hotkey
                    ),
                }
            }
        }

        if self.quick_settings.enabled {
            self.ensure_quick_window(cx);
        } else if let Some(window) = &self.quick_window {
            let _ = window
                .handle
                .update(cx, |_, window, _| hide_quick_terminal(window));
        }
    }

    fn ensure_quick_window(&mut self, cx: &mut App) -> WindowHandle<Root> {
        if let Some(window) = &self.quick_window
            && window.handle.is_active(cx).is_some()
        {
            return window.handle;
        }

        let bounds = Bounds::centered(None, size(px(800.), px(500.0)), cx);
        let window = open_terminal_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitleBar::title_bar_options()),
                focus: false,
                show: false,
                kind: WindowKind::PopUp,
                ..Default::default()
            },
            false,
            cx,
        );
        let handle = window.handle;
        self.quick_window = Some(window);
        handle
    }

    fn activate_normal(&self, cx: &mut App) {
        let _ = self.normal_window.handle.update(cx, |_, window, _| {
            if let Err(error) = show_default_terminal_window(window) {
                eprintln!("Cannot show mightty window: {error}");
                window.activate_window();
            }
        });
    }

    fn dispatch_to_normal(&self, action: AppAction, cx: &mut App) {
        Self::dispatch_to_window(self.normal_window.handle, action, cx);
    }

    fn dispatch_to_window(window: WindowHandle<Root>, action: AppAction, cx: &mut App) {
        let _ = window.update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(DispatchAppAction { action }), cx);
        });
    }

    fn accept_handoff(&mut self, handoff: DefaultTerminalHandoff, cx: &mut App) {
        let replace_existing = self.replace_initial_handoff_tab;
        let (parts, startup_title, response) = handoff.into_parts();
        if self.shutting_down {
            response.send(false);
            return;
        }
        let accepted = self.normal_window.panes.update(cx, |panes, cx| {
            if !panes.can_open_handoff(replace_existing) || !response.send(true) {
                return false;
            }
            panes.open_handoff(parts, startup_title, replace_existing, cx)
        });
        if accepted {
            self.replace_initial_handoff_tab = false;
            self.activate_normal(cx);
        }
    }

    fn shutdown(&mut self) {
        self.shutting_down = true;
        mightty::diagnostics::stop_sampler();
        if self.diagnostic_state.is_object() {
            self.control_revision += 1;
            self.diagnostic_state["revision"] =
                serde_json::json!(self.control_revision.to_string());
            self.diagnostic_state["orderly_shutdown"] = serde_json::json!(true);
            self.diagnostic_state["observed_at"] =
                serde_json::json!(mightty::diagnostics::timestamp());
            self.diagnostic_writer.submit(self.diagnostic_state.clone());
            self.diagnostic_writer.flush(
                &self.control_revision.to_string(),
                std::time::Instant::now() + Duration::from_millis(500),
            );
        }
        self.control_server = None;
        self.default_terminal_server = None;
        self.global_hotkey = None;
        self.primary_instance = None;
        self.activation_tx = None;
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_activation_arguments() {
        assert!(is_embedding_arguments(&[OsString::from("-Embedding")]));
        assert!(is_embedding_arguments(&[OsString::from("/Embedding")]));
        assert!(!is_embedding_arguments(&[OsString::from("--quick")]));
        assert_eq!(
            parse_startup_request(Vec::<OsString>::new()).unwrap(),
            ActivationRequest::Activate
        );
        assert_eq!(
            parse_startup_request(["--profile", "powershell"].map(OsString::from)).unwrap(),
            ActivationRequest::OpenProfile {
                profile_id: ProfileId::new("powershell").unwrap(),
            }
        );
        assert_eq!(
            parse_startup_request(["--quick", "--profile", "wsl:ubuntu"].map(OsString::from))
                .unwrap(),
            ActivationRequest::OpenQuickTerminal {
                profile_id: Some(ProfileId::new("wsl:ubuntu").unwrap()),
            }
        );
        assert_eq!(
            parse_startup_request(["mightty://profile?id=wsl%3Aubuntu"].map(OsString::from))
                .unwrap(),
            ActivationRequest::OpenProfile {
                profile_id: ProfileId::new("wsl:ubuntu").unwrap(),
            }
        );
        assert_eq!(
            parse_startup_request(["mightty://quick?profile=powershell"].map(OsString::from))
                .unwrap(),
            ActivationRequest::OpenQuickTerminal {
                profile_id: Some(ProfileId::new("powershell").unwrap()),
            }
        );
    }

    #[test]
    fn cold_quick_activation_keeps_the_normal_window_hidden() {
        let quick = ActivationRequest::OpenQuickTerminal { profile_id: None };

        assert!(!request_shows_normal_window(&quick));
        assert!(request_shows_normal_window(&ActivationRequest::Activate));
    }

    #[test]
    fn rejects_invalid_windows_activation_arguments() {
        assert!(parse_startup_request(["--unknown"].map(OsString::from)).is_err());
        assert!(parse_startup_request(["--profile", "not valid"].map(OsString::from)).is_err());
        assert!(
            parse_startup_request(["mightty://profile?id=not%20valid"].map(OsString::from))
                .is_err()
        );
        assert!(parse_startup_request(["mightty://quick/extra"].map(OsString::from)).is_err());
        assert!(
            parse_startup_request(["mightty://quick?profile=a&extra=b"].map(OsString::from))
                .is_err()
        );
    }
}
