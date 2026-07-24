# mightty Product Requirements

## Product Goal

Build a complete, fast terminal emulator suitable for daily use. mightty is
Windows-first, but its terminal engine and shell boundary must remain portable
to Unix platforms.

The current tabs, panes, renderer, and PTY bridge are the foundation of the
product. They are not the intended ceiling. Missing interaction, configuration,
session, and terminal features are product work, not permanent non-goals.

## Product Principles

- Terminal correctness comes first. Ghostty owns VT parsing, terminal state,
  scrollback, Unicode cell width, terminal modes, and key encoding.
- mightty owns the native application: rendering, input translation, PTY
  lifecycle, tabs, panes, settings, and platform integration.
- Ghostty source is the only terminal-engine upstream. The project does not
  depend on a separately maintained Ghostty Rust crate.
- Raw C bindings remain private and complete; safe Rust APIs expand as terminal
  features are integrated.
- Windows is the primary product target. Unix support stays behind the same
  shell interface and must not be coupled to ConPTY details.
- Error paths, process cleanup, resize behavior, and sustained output handling
  are product requirements, not secondary polish.

## Current Product Baseline

### Terminal Engine and Rendering

- The pinned `ghostty/` submodule is built as static `libghostty-vt` with Zig
  `0.16.0`.
- Project-owned Rust wrappers provide terminal ownership, PTY callbacks, render
  snapshots, cell iteration, styles, and key encoding.
- GPUI renders cell-positioned terminal text with embedded JetBrainsMono Nerd
  Font Mono faces and platform font fallback.
- The renderer supports terminal foreground/background colors, bold, italic,
  underline, strikethrough, wide cells, cursor styles, and cursor blinking.
- `Ctrl+Shift+F12` records semantic terminal state as JSON. Windows also
  attempts a PNG capture of the client area.

### Shell and Process I/O

- Windows uses ConPTY; Unix uses forkpty.
- PTY input, resize commands, terminal replies, blocking output reads, process
  exit, and shutdown are separated behind one platform-neutral shell boundary.
- The default shell is `pwsh.exe` on Windows. Unix uses `$SHELL` and falls back
  to `/bin/sh`.
- Output is bounded between the reader thread and GPUI and drained in batches
  so sustained shell output does not create an unbounded UI queue.

### Tabs and Panes

- `Ctrl+T` creates a tab; `Cmd+T` does the same on macOS.
- The app currently supports at most nine tabs, addressable with `Ctrl+1`
  through `Ctrl+9`.
- Tabs appear in a `160px` left sidebar that `Ctrl+B` toggles.
- A tab title is captured from the app process's current directory when the tab
  is created. It uses `~` for the home directory and `term` if no usable name
  is available.
- `Alt+Enter` splits right and `Alt+Shift+Enter` splits down. New equal-flex
  panes become active and are separated by a one-pixel green line.
- `Ctrl+D` closes the active pane, closes a single-pane tab, or quits when only
  one tab with one pane remains. The shortcut is not sent to the shell.
- Closing a pane collapses single-child split nodes and focuses a remaining
  pane. Closing a tab prefers the previous tab.
- An exited pane is removed when its tab has another pane. A tab's last exited
  pane currently remains visible.

## Required Product Areas

The terminal is not feature-complete until these areas are deliberately
designed and implemented:

- scrollback navigation and viewport state;
- text selection, copy, paste, bracketed paste, and clipboard integration;
- mouse reporting, links, drag behavior, and terminal-aware pointer handling;
- IME/composition, dead keys, international keyboard layouts, and
  accessibility;
- shell profiles, working-directory-aware titles, themes, fonts, settings, and
  user-configurable key bindings;
- interactive pane resizing, tab/pane reordering, and richer pane lifecycle;
- search, command/session affordances, and optional session persistence;
- complete integration of Ghostty terminal features such as mouse encoding,
  selection, hyperlinks, and graphics where they fit the product;
- startup diagnostics, recoverable runtime errors, performance measurement,
  packaging, updates, and release workflows.

This list describes product scope, not an implementation order.

## Architecture Constraints

- `PaneContainer` owns tabs, sidebar state, top-level actions, and app
  shortcuts.
- Each tab owns one root `Split`; `Split` directly owns
  `Entity<TerminalWidget>` leaves.
- `TerminalWidget` coordinates safe Ghostty state, render iterators, key
  encoding, feedback capture, and a `PtyWorker`.
- `src/shell/` owns platform handles and child-process cleanup.
- `src/ghostty/` is the only Rust/C boundary. Unsafe operations, C ownership,
  and lending lifetimes stay inside it.
- `build.rs` builds only the pinned submodule and refuses a source revision or
  header fingerprint that does not match `bindings.version`.

## Quality Gates

- Terminal input, output, resize, and shutdown paths must be tested in the
  closest practical environment to the operating system.
- Changes to Ghostty source and generated bindings are reviewed together.
- Binding tests compare Rust layouts with target-native Ghostty metadata.
- Formatting, checking, linting, tests, and a release build must pass:

```bash
mise exec -- cargo fmt --all -- --check
mise exec -- cargo check
mise exec -- cargo clippy --all-targets -- -D warnings
mise exec -- cargo test
mise exec -- cargo build --release
```
