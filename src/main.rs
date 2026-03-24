use gpui::{prelude::*, px, size, App, Application, Bounds, WindowBounds, WindowOptions};
use gpui_component::Root;

mod ghostty;
mod widget;

#[cfg(windows)]
mod shell;

use std::time::Duration;
use widget::{TerminalConfig, TerminalWidget};

fn main() {
    // Run the GPUI application with TerminalWidget
    Application::new().run(|cx: &mut App| {
        // Initialize gpui-component
        gpui_component::init(cx);

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
                let view = cx.new(|cx| TerminalWidget::new(config, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
