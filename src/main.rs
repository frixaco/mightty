use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Render, Window, WindowBounds,
    WindowOptions,
};
use gpui_component::{button::*, Root, StyledExt};

// Ghostty terminal module
mod ghostty;
use ghostty::Terminal;

#[cfg(windows)]
mod shell;
#[cfg(windows)]
use shell::ConPtyShell;

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

#[cfg(windows)]
fn test_conpty_shell() {
    println!("\n=== Testing ConPTY Shell Bridge ===\n");

    // Spawn cmd.exe
    let mut shell = match ConPtyShell::spawn("cmd.exe", 24, 80) {
        Ok(s) => {
            println!("✓ Spawned cmd.exe successfully");
            s
        }
        Err(e) => {
            println!("✗ Failed to spawn shell: {}", e);
            return;
        }
    };

    // Read initial prompt (give it time to start)
    std::thread::sleep(std::time::Duration::from_millis(500));

    let mut buf = [0u8; 4096];
    match shell.read(&mut buf) {
        Ok(n) if n > 0 => {
            let output = String::from_utf8_lossy(&buf[..n]);
            println!("✓ Read {} bytes from shell", n);
            println!(
                "  Initial output: {:?}",
                output.chars().take(100).collect::<String>()
            );
        }
        Ok(_) => println!("  (No initial output)"),
        Err(e) => println!("✗ Read error: {}", e),
    }

    // Write a command
    println!("\n  Writing command: echo Hello from ConPTY");
    if let Err(e) = shell.write(b"echo Hello from ConPTY\r\n") {
        println!("✗ Write error: {}", e);
        return;
    }

    // Read the command output
    std::thread::sleep(std::time::Duration::from_millis(200));

    match shell.read(&mut buf) {
        Ok(n) if n > 0 => {
            let output = String::from_utf8_lossy(&buf[..n]);
            println!("✓ Read {} bytes response", n);

            // Check if our expected text is in the output
            if output.contains("Hello from ConPTY") {
                println!("✓ Command executed successfully!");
            } else {
                println!(
                    "  Response: {:?}",
                    output.chars().take(200).collect::<String>()
                );
            }
        }
        Ok(_) => println!("  (No response)"),
        Err(e) => println!("✗ Read error: {}", e),
    }

    // Test resize
    println!("\n  Testing resize to 30x100...");
    match shell.resize(30, 100) {
        Ok(_) => println!("✓ Resized successfully"),
        Err(e) => println!("✗ Resize failed: {}", e),
    }

    // Write another command after resize
    println!("\n  Writing command: echo Resized terminal");
    if shell.write(b"echo Resized terminal\r\n").is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(200));

        match shell.read(&mut buf) {
            Ok(n) if n > 0 => {
                let output = String::from_utf8_lossy(&buf[..n]);
                if output.contains("Resized terminal") {
                    println!("✓ Resize + command working!");
                }
            }
            _ => {}
        }
    }

    // Shutdown
    println!("\n  Shutting down shell...");
    if let Err(e) = shell.shutdown() {
        println!("✗ Shutdown error: {}", e);
    } else {
        println!("✓ Shell shutdown successfully");
    }

    println!("\n=== ConPTY Shell Bridge test complete ===\n");
}

fn main() {
    // Test the ghostty terminal first
    test_ghostty_terminal();

    // Test ConPTY shell bridge (Windows only)
    #[cfg(windows)]
    test_conpty_shell();

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
