# AGENTS.md

## Project Overview

mightty is a Windows-first GPU-powered terminal emulator intended to grow into
a complete terminal product. The current tabs and panes are a foundation, not
the product boundary.

Core pieces:
- **GPUI** and **gpui-component** for the application shell and rendering.
- **Ghostty's libghostty-vt**, built from the source submodule, for terminal
  state, escape sequence handling, rendering snapshots, and key encoding.
- A project-owned safe Rust module over Ghostty's private generated C bindings.
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
│   ├── input.rs         # GPUI key event to Ghostty key encoding
│   ├── render.rs        # Terminal cell rendering
│   └── capture.rs       # Terminal-state feedback snapshot
├── ghostty/
│   ├── mod.rs           # Public local Ghostty interface
│   ├── terminal.rs      # Terminal ownership and PTY callback
│   ├── selection.rs     # Ghostty selection gesture state and event bridge
│   ├── render.rs        # Snapshot and lending render iterators
│   ├── key.rs           # Key event and encoder ownership
│   ├── style.rs         # Renderer-facing colors and styles
│   ├── error.rs         # C result conversion
│   ├── abi.rs           # Target-native Rust/Zig layout audit
│   ├── bindings.version # Expected Ghostty revision/header fingerprint
│   └── ffi.rs           # Generated private C bindings
└── shell/
    ├── mod.rs           # Platform shell bridge exports
    ├── windows.rs       # ConPTY implementation
    └── unix.rs          # forkpty implementation
```

Important non-source paths:
- `ghostty/`: the only Ghostty source checkout; pinned as a git submodule.
- `tools/ghostty-bindings/`: reproducible generator for `src/ghostty/ffi.rs`.
- `fonts/JetBrainsMono/`: ignored local font files embedded by `src/main.rs`;
  the regular, bold, italic, and bold-italic mono faces are required.
- `captures/`: generated at runtime by feedback capture and intentionally not part of source.

## Build System

The root `build.rs` invokes Zig directly against the local `ghostty/` submodule,
then statically links `libghostty-vt`. The binding generator records the
submodule revision and public-header fingerprint in
`src/ghostty/bindings.version`; every build verifies the checkout against that
record. Cargo does not fetch Ghostty and the repo does not depend on the
`libghostty-vt` or `libghostty-vt-sys` Rust crates.

Ghostty's build requires Zig. The repo includes `.mise.toml` pinning Zig
`0.16.0`; `ZIG=/path/to/zig` can override discovery. Git is also a build
requirement because `build.rs` validates the submodule revision.

## Behavior

- Default shell command is `pwsh.exe` on Windows. Unix uses `$SHELL` and falls
  back to `/bin/sh`.
- `Ctrl+T` creates a new tab; `Cmd+T` does the same on macOS.
- The app supports at most nine tabs.
- Tabs appear in a left sidebar and can be selected with `Ctrl+1` through `Ctrl+9`.
- `Ctrl+B` toggles the sidebar.
- `Alt+Enter` splits the active pane to the right.
- `Alt+Shift+Enter` splits the active pane downward.
- `Ctrl+D` closes the active pane, or closes the active tab when it has one pane.
- The mouse wheel scrolls terminal history.
- Left-button drag selects text; double-click selects a word and triple-click
  selects a line using Ghostty's selection rules.
- `Ctrl+Alt` drag selects a rectangle (`Option` on macOS).
- `Ctrl+Shift+C` copies the active selection; `Cmd+C` does the same on macOS.
- `Cmd+Q` quits on macOS.
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

Rendering snapshots use the local `RenderState`, `RowIterator`, and
`CellIterator` wrappers.
The row and cell iterations are lending iterators, so drive them with `while let Some(row) = rows.next()`.

## Development Guidelines

- Keep code simple and local. Do not add abstraction unless it removes real complexity.
- Keep raw bindings private in `src/ghostty/ffi.rs`; regenerate them from the
  exact submodule headers instead of editing them.
- Keep unsafe C calls and ownership rules inside `src/ghostty/`. Expose only the
  terminal concepts through safe wrappers, and preserve the non-`Send`/non-`Sync`
  and lending-iterator invariants. Keep generated bindings complete so new
  Ghostty features do not require generator allowlist changes.
- Update `src/ghostty/bindings.version` and `ffi.rs` together with every
  Ghostty source update by running the binding generator.
- Be careful with Windows handles in `src/shell/windows.rs`; every failure path must close owned handles.
- Avoid broad UI rewrites unless the task explicitly asks for product design work.
- Keep docs accurate to actual behavior.
- Run formatting and checks prior to handoff.

## Common Commands

```bash
git submodule update --init ghostty
mise install
mise exec -- cargo fmt --all -- --check
mise exec -- cargo check
mise exec -- cargo clippy --all-targets -- -D warnings
mise exec -- cargo test
mise exec -- cargo run
mise exec -- cargo build --release

# Requires libclang; run after changing the Ghostty revision or headers.
mise exec -- cargo run --manifest-path tools/ghostty-bindings/Cargo.toml
```
