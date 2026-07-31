use gpui::{
    App, Application, Bounds, WindowBounds, WindowHandle, WindowKind, WindowOptions, prelude::*,
    px, size,
};
use gpui_component::{Root, Theme, ThemeMode, TitleBar};
use mightty::{pane_container::PaneContainer, settings::SettingsStore};
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
    #[cfg(windows)]
    let Some(windows_startup) = windows_startup() else {
        return;
    };

    Application::new().run(move |cx: &mut App| {
        load_embedded_fonts(cx);
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

        #[cfg(windows)]
        {
            let normal_window = open_normal_window(windows_startup.show_normal_window(), cx);
            start_windows_application(windows_startup, normal_window, cx);
        }
        #[cfg(not(windows))]
        let _normal_window = open_normal_window(true, cx);

        cx.activate(true);
    });
}

struct TerminalWindow {
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
        cx,
    )
}

fn open_terminal_window(options: WindowOptions, cx: &mut App) -> TerminalWindow {
    let settings = SettingsStore::open_default();
    let panes = cx.new(|cx| PaneContainer::new(settings, cx));
    let root_panes = panes.clone();
    let handle = cx
        .open_window(options, move |window, cx| {
            cx.new(|cx| Root::new(root_panes, window, cx))
        })
        .expect("failed to open terminal window");
    TerminalWindow { handle, panes }
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
}

#[cfg(windows)]
impl WindowsStartup {
    fn show_normal_window(&self) -> bool {
        match self {
            Self::Application { request, .. } => request_shows_normal_window(request),
            Self::Embedding => false,
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
fn start_windows_application(startup: WindowsStartup, normal_window: TerminalWindow, cx: &mut App) {
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
    };
    let (handoff_tx, handoff_rx) = flume::unbounded();
    let default_terminal_server = DefaultTerminalServer::start(handoff_tx)
        .expect("failed to start the default-terminal COM server");

    let controller = Rc::new(RefCell::new(WindowsApplication {
        normal_window,
        quick_window: None,
        settings: SettingsStore::open_default(),
        quick_settings: QuickTerminalSettings::default(),
        activation_tx: (!embedding).then_some(activation_tx),
        primary_instance,
        global_hotkey: None,
        default_terminal_server: Some(default_terminal_server),
        embedding,
        received_handoff: false,
    }));
    controller.borrow_mut().apply_settings(cx);

    let action_controller = Rc::clone(&controller);
    cx.on_action::<DispatchAppAction>(move |action, cx| {
        if action.action == AppAction::ToggleQuickTerminal {
            action_controller.borrow_mut().dispatch(
                ActivationRequest::Dispatch {
                    action: action.action.clone(),
                },
                cx,
            );
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
    cx.on_app_quit(move |_| {
        shutdown_controller.borrow_mut().shutdown();
        async {}
    })
    .detach();

    if let Some(startup_request) = startup_request {
        controller.borrow_mut().dispatch(startup_request, cx);
    }
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
    embedding: bool,
    received_handoff: bool,
}

#[cfg(windows)]
impl WindowsApplication {
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
                eprintln!("Application settings reload failed: {diagnostic}");
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

        if !self.quick_settings.enabled
            && let Some(window) = &self.quick_window
        {
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
        let replace_existing = self.embedding && !self.received_handoff;
        let (parts, startup_title, response) = handoff.into_parts();
        let accepted = self.normal_window.panes.update(cx, |panes, cx| {
            panes.open_handoff(parts, startup_title, replace_existing, cx)
        });
        response.send(accepted);
        if accepted {
            self.received_handoff = true;
            self.activate_normal(cx);
        }
    }

    fn shutdown(&mut self) {
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
        assert!(!request_shows_normal_window(
            &ActivationRequest::OpenQuickTerminal { profile_id: None }
        ));
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
