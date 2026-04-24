# mightty

mightty is a small GPU-powered terminal emulator prototype built with Rust, GPUI, Windows ConPTY, and Ghostty's `libghostty-vt`.

It is currently Windows-first. Unix shell support is represented by a placeholder module, but PTY integration is not implemented yet.

## Features

- GPU-rendered terminal UI through GPUI.
- Terminal emulation through `libghostty-vt`.
- Windows shell I/O through ConPTY.
- Multiple side-by-side panes with `Alt+Enter`.
- Embedded JetBrainsMono Nerd Font Mono for terminal text.
- Feedback capture with `Ctrl+Shift+F12`.

## Stack

- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) for UI rendering.
- [gpui-component](https://crates.io/crates/gpui-component) for the root component wrapper.
- [Ghostty](https://github.com/ghostty-org/ghostty) for `libghostty-vt`.
- Windows ConPTY for shell process integration.

## Requirements

- Rust with edition 2024 support.
- Zig `0.15.2`.
- The Ghostty submodule initialized at `ghostty/`.
- Windows for the runnable shell bridge.

This repo includes a `.mise.toml` pin for Zig:

```bash
mise install
```

You can also set `ZIG` to a specific Zig executable.

Pinned Ghostty commit:

```text
b0d359cbbd945f9f3807327526ef79fcaf0477df
```

## Build

```bash
cargo build
cargo build --release
```

`build.rs` validates Zig, builds the local `ghostty/` checkout with:

```bash
zig build -Demit-lib-vt=true -Dsimd=false
```

On Windows, the build copies `ghostty-vt.dll` into Cargo's target output directory so the app and tests can load it.

## Run

```bash
cargo run
```

The default shell is `pwsh.exe`.

## Development

Useful checks:

```bash
cargo fmt
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
```

Useful runtime shortcuts:

- `Alt+Enter`: open a new pane to the right.
- `Ctrl+Shift+F12`: write a terminal feedback capture to `captures/`.

## TODO

- Implement a Unix PTY backend behind the existing `PtySession` API for macOS/Linux, using a nonblocking PTY master for reads, writes, resize, child exit detection, and shell spawning from `$SHELL`.
- Remove the temporary `ConPtyShell` and `ConPtyError` compatibility aliases once all callers use the platform-neutral PTY names.

## Project Layout

```text
src/
├── main.rs              # App entry point and window setup
├── lib.rs               # Library module exports
├── feedback.rs          # Feedback capture output
├── pane.rs              # Single pane wrapper
├── pane_container.rs    # Pane actions and key binding
├── split.rs             # Side-by-side pane layout
├── widget/mod.rs        # Terminal widget, rendering, input, shell I/O thread
├── ghostty/mod.rs       # Safe wrapper around used libghostty-vt APIs
└── shell/
    ├── windows.rs       # Windows ConPTY implementation
    └── unix.rs          # Unsupported placeholder
```

## License

MIT
