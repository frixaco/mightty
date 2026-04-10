use gpui::{prelude::*, px, size, App, Application, Bounds, WindowBounds, WindowOptions};
use gpui_component::Root;

mod ghostty;
mod widget;
mod pane;
mod split;
mod pane_container;

#[cfg(windows)]
mod shell;

use std::time::Duration;
use widget::TerminalConfig;
use pane_container::PaneContainer;

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);

        PaneContainer::bind_keys(cx);

        let bounds = Bounds::centered(None, size(px(800.), px(600.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let config = TerminalConfig {
                    shell: "pwsh.exe".to_string(),
                    initial_rows: 30,
                    initial_cols: 100,
                    scrollback: 10000,
                    cursor_blink: true,
                    blink_interval: Duration::from_millis(500),
                    ..Default::default()
                };
                let pane_container = cx.new(|cx| PaneContainer::new(config, cx));
                cx.new(|cx| Root::new(pane_container, window, cx))
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
