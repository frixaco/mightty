/// Simple example demonstrating libghostty-vt usage
use mightty::ghostty::{GhosttyTerminalOptions, Terminal};

fn main() {
    println!("=== libghostty-vt Terminal Example ===\n");

    // Create a new terminal with custom size
    let mut terminal = Terminal::new(GhosttyTerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 1000,
    })
    .expect("Failed to create terminal");

    println!("✓ Terminal created");

    // Check initial size
    let (cols, rows) = terminal.size();
    println!("✓ Initial size: {} columns x {} rows", cols, rows);

    // Write some text to the terminal
    terminal.write_str("Hello from libghostty!\r\n");
    terminal.write_str("This is line 2.\r\n");
    terminal.write_str("Line 3 with some text here.\r\n");
    println!("✓ Written 3 lines to terminal");

    // Check cursor position
    let (x, y) = terminal.cursor_pos();
    println!("✓ Cursor position: column {}, row {}", x, y);

    // Check cursor visibility
    let visible = terminal.cursor_visible();
    println!("✓ Cursor visible: {}", visible);

    // Test resize
    terminal.resize(100, 50).expect("Failed to resize");
    let (new_cols, new_rows) = terminal.size();
    println!("✓ Resized to: {} columns x {} rows", new_cols, new_rows);

    // Check scrollbar state
    if let Some(sb) = terminal.scrollbar() {
        println!("✓ Scrollbar state:");
        println!("  - Total rows: {}", sb.total);
        println!("  - Current offset: {}", sb.offset);
        println!("  - Visible rows: {}", sb.len);
    } else {
        println!("✗ Failed to get scrollbar state");
    }

    // Test reset
    terminal.reset();
    println!("✓ Terminal reset to initial state");

    // Verify size after reset
    let (reset_cols, reset_rows) = terminal.size();
    println!(
        "✓ Size after reset: {} columns x {} rows",
        reset_cols, reset_rows
    );

    println!("\n=== Example complete ===");
}
