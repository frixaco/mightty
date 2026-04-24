# AGENTS.md

## Project Overview

mightty is a small GPU-powered terminal emulator prototype.

Core pieces:
- **GPUI** and **gpui-component** for the application shell and rendering.
- **libghostty-vt** for terminal state, escape sequence handling, rendering snapshots, and key encoding.
- **Windows ConPTY** for shell process I/O.
- Embedded **JetBrainsMono Nerd Font Mono** assets for terminal text rendering.

This is currently Windows-first. The `shell` module has a Unix placeholder so the module boundary is explicit, but Unix PTY support is not implemented.

## Source Layout

```text
src/
├── main.rs              # Binary entry point, window setup, font loading
├── lib.rs               # Library module exports
├── feedback.rs          # JSON and PNG feedback capture support
├── pane.rs              # Single terminal pane wrapper
├── pane_container.rs    # Top-level pane container and key bindings
├── split.rs             # Simple row of terminal panes
├── widget/
│   └── mod.rs           # TerminalWidget: rendering, input, shell bridge thread
├── ghostty/
│   └── mod.rs           # Minimal safe wrapper around used libghostty-vt APIs
└── shell/
    ├── mod.rs           # Platform shell bridge exports
    ├── windows.rs       # ConPTY implementation
    └── unix.rs          # Unsupported placeholder
```

Important non-source paths:
- `ghostty/`: local Ghostty checkout used by `build.rs`.
- `fonts/JetBrainsMono/`: embedded font files referenced by `src/main.rs`.
- `captures/`: generated at runtime by feedback capture and intentionally not part of source.

## Build System

`build.rs` expects the Ghostty submodule checkout at `ghostty/`.

The build script:
1. Validates Zig `0.15.2`.
2. Runs Ghostty's Zig build with `-Demit-lib-vt=true -Dsimd=false`.
3. Installs artifacts under Cargo's `OUT_DIR`.
4. On Windows, copies `ghostty-vt.dll` into the target output directory and `deps/` when present.
5. On non-Windows targets, emits static link directives for `ghostty-vt`.

The repo includes `.mise.toml` pinning Zig `0.15.2`. `ZIG=/path/to/zig` can override discovery.

## Current Behavior

- Default shell command is `pwsh.exe`.
- `Alt+Enter` opens another pane to the right.
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

Rendering snapshots use `RenderState`, `RowIterator`, and `CellIterator`. `Rows` and `Cells` implement `Iterator`.

## Development Guidelines

- Keep code simple and local. Do not add abstraction unless it removes real complexity.
- Preserve the thin FFI boundary in `src/ghostty/mod.rs`; expose only APIs the app uses.
- Be careful with Windows handles in `src/shell/windows.rs`; every failure path must close owned handles.
- Avoid broad UI rewrites unless the task explicitly asks for product design work.
- Keep docs accurate to implemented behavior. Do not document planned tabs, horizontal splits, or Unix PTY support as shipped features.
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
