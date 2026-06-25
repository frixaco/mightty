# mightty

mightty is a small GPU-powered terminal emulator prototype built with Rust, GPUI, platform PTYs, and Ghostty's `libghostty-vt`.

It is currently Windows-first, with Windows shell I/O through ConPTY and Unix shell I/O through a forkpty-backed bridge.

## Features

- GPU-rendered terminal UI through GPUI.
- Terminal emulation through `libghostty-vt`.
- Windows shell I/O through ConPTY.
- Unix shell I/O through forkpty.
- Tabs in a compact left sidebar.
- Right and down pane splits.
- Active pane and tab closing.
- Embedded JetBrainsMono Nerd Font Mono for terminal text.
- Feedback capture with `Ctrl+Shift+F12`.

## Stack

- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) for UI rendering.
- [gpui-component](https://crates.io/crates/gpui-component) for the root component wrapper.
- [libghostty-vt](https://crates.io/crates/libghostty-vt) for the Ghostty VT engine.
- Windows ConPTY and Unix forkpty for shell process integration.

## Requirements

- Rust with edition 2024 support.
- Zig `0.15.2`.
- The Ghostty checkout at `ghostty/`, used by `.cargo/config.toml` as `GHOSTTY_SOURCE_DIR`.
- Local JetBrainsMono Nerd Font Mono files under `fonts/JetBrainsMono/`.
- Windows for the primary shell bridge target, or a Unix platform for the forkpty bridge.

This repo includes a `.mise.toml` pin for Zig:

```bash
mise install
```

You can also set `ZIG` to a specific Zig executable.

## Build

```bash
cargo build
cargo build --release
```

The `libghostty-vt-sys` dependency builds Ghostty's VT library during Cargo builds.
This repo's `.cargo/config.toml` points `GHOSTTY_SOURCE_DIR` at the local
`ghostty/` checkout so normal Cargo commands do not need to fetch Ghostty.

```powershell
cargo build
```

## Run

```bash
cargo run
```

The default shell is `pwsh.exe` on Windows and `$SHELL` on Unix.

## Development

Useful checks:

```bash
cargo fmt
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
```

Useful runtime shortcuts:

- `Ctrl+T`: open a new tab.
- `Ctrl+B`: hide or show the tab sidebar.
- `Ctrl+1` through `Ctrl+9`: switch to an existing tab.
- `Alt+Enter`: split the active pane to the right.
- `Alt+Shift+Enter`: split the active pane downward.
- `Ctrl+D`: close the active pane, or close the active tab when it has one pane.
- `Ctrl+Shift+F12`: write a terminal feedback capture to `captures/`.

## Project Layout

```text
src/
├── main.rs              # App entry point and window setup
├── lib.rs               # Library module exports
├── feedback.rs          # Feedback capture output
├── pane_container.rs    # Tabs, sidebar, pane actions, and key bindings
├── split.rs             # Orientation-aware terminal split tree
├── widget/
│   ├── mod.rs           # Terminal widget lifecycle and GPUI task wiring
│   ├── pty.rs           # Wake-driven PTY worker bridge
│   ├── input.rs         # GPUI key event to libghostty-vt key encoding
│   ├── render.rs        # Terminal cell rendering
│   └── capture.rs       # Terminal-state feedback snapshot
├── ghostty/mod.rs       # Project facade over the libghostty-vt crate
└── shell/
    ├── windows.rs       # Windows ConPTY implementation
    └── unix.rs          # Unix forkpty implementation
```

## License

MIT
