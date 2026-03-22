use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Render, Window, WindowBounds,
    WindowOptions,
};
use gpui_component::{button::*, Root, StyledExt};

// Ghostty terminal module
mod ghostty;
use ghostty::Terminal;

struct HelloWorld;

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .size_full()
            .items_center()
            .justify_center()
            .child("Hello, World!")
            .child(
                Button::new("ok")
                    .primary()
                    .label("Let's Go!")
                    .on_click(|_, _, _| println!("Clicked!")),
            )
    }
}

fn test_ghostty_terminal() {
    println!("\n=== Testing libghostty-vt ===\n");

    // Create a new terminal (80x24, 1000 scrollback lines)
    let mut terminal = match Terminal::new_default() {
        Ok(t) => {
            println!("✓ Terminal created successfully");
            t
        }
        Err(e) => {
            println!("✗ Failed to create terminal: {:?}", e);
            return;
        }
    };

    // Check initial size
    let (cols, rows) = terminal.size();
    println!("✓ Initial size: {}x{}", cols, rows);

    // Write some data to the terminal
    terminal.write_str("Hello from libghostty!\r\n");
    terminal.write_str("This is a test of the terminal emulator.\r\n");
    println!("✓ Written test data to terminal");

    // Check cursor position
    let (x, y) = terminal.cursor_pos();
    println!("✓ Cursor position: ({}, {})", x, y);

    // Check cursor visibility
    let visible = terminal.cursor_visible();
    println!("✓ Cursor visible: {}", visible);

    // Check scrollbar state
    if let Some(sb) = terminal.scrollbar() {
        println!(
            "✓ Scrollbar: total={}, offset={}, len={}",
            sb.total, sb.offset, sb.len
        );
    }

    // Test resize
    if terminal.resize(100, 50).is_ok() {
        let (new_cols, new_rows) = terminal.size();
        println!("✓ Resized to: {}x{}", new_cols, new_rows);
    }

    // Test reset
    terminal.reset();
    println!("✓ Terminal reset");

    println!("\n=== libghostty-vt test complete ===\n");
}

fn main() {
    // Test the ghostty terminal first
    test_ghostty_terminal();

    // Then run the GPUI application
    Application::new().run(|cx: &mut App| {
        // Initialize gpui-component
        gpui_component::init(cx);

        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|_| HelloWorld);
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
