# AGENTS.md

## Project Overview

mightty is a small GPU-powered terminal emulator prototype.

Core pieces:
- **GPUI** and **gpui-component** for the application shell and rendering.
- **libghostty-vt** for terminal state, escape sequence handling, rendering snapshots, and key encoding.
- **Windows ConPTY** and **Unix forkpty** for shell process I/O.
- Local **JetBrainsMono Nerd Font Mono** assets for embedded terminal text rendering.

Windows remains the primary target, with a Unix shell bridge available behind the same shell module boundary.

## Source Layout

```text
src/
├── main.rs              # Binary entry point, window setup, font loading
├── lib.rs               # Library module exports
├── feedback.rs          # JSON and PNG feedback capture support
├── pane_container.rs    # Tabs, sidebar, top-level pane actions, and key bindings
├── split.rs             # Orientation-aware terminal split tree
├── widget/
│   ├── mod.rs           # TerminalWidget lifecycle and GPUI task wiring
│   ├── pty.rs           # Wake-driven PTY worker bridge
│   ├── input.rs         # GPUI key event to libghostty-vt key encoding
│   ├── render.rs        # Terminal cell rendering
│   └── capture.rs       # Terminal-state feedback snapshot
├── ghostty/
│   └── mod.rs           # Project facade over the libghostty-vt crate
└── shell/
    ├── mod.rs           # Platform shell bridge exports
    ├── windows.rs       # ConPTY implementation
    └── unix.rs          # forkpty implementation
```

Important non-source paths:
- `ghostty/`: local Ghostty checkout used by `.cargo/config.toml` as `GHOSTTY_SOURCE_DIR`.
- `fonts/JetBrainsMono/`: local font files embedded by `src/main.rs`.
- `captures/`: generated at runtime by feedback capture and intentionally not part of source.

## Build System

The `libghostty-vt-sys` dependency builds Ghostty's VT library through Cargo.
The repo's `.cargo/config.toml` sets `GHOSTTY_SOURCE_DIR` to the local `ghostty/`
checkout so normal Cargo commands do not need to fetch Ghostty.

The repo includes `.mise.toml` pinning Zig `0.15.2`. `ZIG=/path/to/zig` can override discovery.

## Current Behavior

- Default shell command is `pwsh.exe` on Windows and `$SHELL` on Unix.
- `Ctrl+T` creates a new tab.
- Tabs appear in a left sidebar and can be selected with `Ctrl+1` through `Ctrl+9`.
- `Ctrl+B` toggles the sidebar.
- `Alt+Enter` splits the active pane to the right.
- `Alt+Shift+Enter` splits the active pane downward.
- `Ctrl+D` closes the active pane, or closes the active tab when it has one pane.
- Exited panes are removed when more than one pane exists.
- `Ctrl+Shift+F12` writes a feedback capture under `captures/`.
  - JSON terminal-state capture is cross-platform.
  - PNG window capture is Windows-only.

## Key APIs

Terminal setup:

```rust
use mightty::ghostty::{Terminal, TerminalOptions};

let mut terminal = Terminal::new(TerminalOptions {
    rows: 24,
    cols: 80,
    max_scrollback: 1000,
})?;

terminal.vt_write(b"hello");
terminal.resize(80, 24, 10, 20)?;
```

Rendering snapshots use `RenderState`, `RowIterator`, and `CellIterator` from `libghostty-vt`.
The row and cell iterations are lending iterators, so drive them with `while let Some(row) = rows.next()`.

## Development Guidelines

- Keep code simple and local. Do not add abstraction unless it removes real complexity.
- Keep `src/ghostty/mod.rs` as a small facade over `libghostty-vt`; do not reintroduce manual FFI bindings.
- Be careful with Windows handles in `src/shell/windows.rs`; every failure path must close owned handles.
- Avoid broad UI rewrites unless the task explicitly asks for product design work.
- Keep docs accurate to implemented behavior.
- Run formatting and checks before handing off.

## Common Commands

```bash
mise install
cargo fmt
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run
cargo build --release
```
