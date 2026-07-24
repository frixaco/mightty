# Next feature roadmap

This document records the next substantial product features for mightty and the
architecture that should support them. The order is intentional: later features
depend on the interaction, configuration, and action foundations above them.

## Short feature list

1. **Complete terminal interaction:** scrollback, selection, copy/paste, mouse
   reporting, hyperlinks, a scrollbar, and in-pane search.
2. **Add profiles and settings:** shell profiles, themes, fonts, launch
   directories, environment variables, key bindings, and live reload.
3. **Introduce typed actions and a command palette:** one action model shared by
   shortcuts, menus, the palette, and future automation.
4. **Finish pane and workspace management:** directional focus, resize, zoom,
   movable tabs, named workspaces, and restorable layouts.
5. **Add shell integration and semantic commands:** dynamic titles and working
   directories, prompt navigation, command-output selection, and notifications.
6. **Build the Windows product experience:** a global quick-terminal window,
   single-instance activation, installer, updater, and default-terminal
   integration.
7. **Render Kitty graphics:** images placed above or below terminal text, using
   Ghostty's existing protocol implementation.

## Architectural direction

Use each dependency at the boundary where it is strongest:

- **Ghostty owns terminal semantics.** Extend the safe Rust module under
  `src/ghostty/` instead of recreating selection, mouse encoding, paste safety,
  shell markers, search, or image-protocol state in GPUI code.
- **`TerminalWidget` adapts GPUI events and renders terminal state.** It should
  not become the owner of application actions, workspace topology, settings, or
  Windows lifecycle services.
- **`PaneContainer` and `Split` own product topology.** Keep PTYs and GPUI
  entities out of serialized workspace state.
- **Typed actions connect every invocation surface.** A shortcut, menu item,
  palette entry, and IPC request should dispatch the same action value.
- **Windows-specific behavior stays behind small services.** ConPTY remains in
  `src/shell/windows.rs`; global hotkeys, activation IPC, monitor placement, and
  installation belong at the application layer.

This is the useful synthesis of the reference projects:

- Ghostty has the cleanest terminal-engine boundary: its `Surface` adapts
  platform events while terminal state and protocol rules remain in the engine.
- WezTerm has the strongest typed action model and geometry-aware pane tree.
- Kitty shows how action metadata can power discoverability and how optional
  experiences can remain separate tools or special windows instead of bloating
  the terminal core.

## 1. Complete terminal interaction

### Implementation

Add safe, responsibility-focused wrappers under `src/ghostty/` for:

- viewport scrolling and scrollbar data;
- selection gestures, tracked selection ranges, and formatted selected text;
- terminal mouse modes and mouse encoding;
- bracketed-paste encoding and dangerous-paste detection;
- hyperlinks, title, working directory, bell, and clipboard callbacks.

`TerminalWidget` should translate a GPUI pointer position to a cell position
once, then route the event according to Ghostty's current mouse-tracking mode:
terminal mouse reporting when the application requests it, local selection
otherwise. Holding Shift should temporarily select locally, matching established
terminal behavior.

Render selection as visible row ranges, not as a separate FFI query for every
cell. Scrollbar data can be polled once per render or terminal write batch; the
Ghostty API is designed for that access pattern. Copy is an explicit user
action. Application-initiated clipboard writes such as OSC 52 need a separate
policy because terminal output is untrusted.

Paste must pass through Ghostty's safety check and encoder. Unsafe multiline or
control-sequence paste opens a GPUI confirmation overlay before any bytes reach
the PTY. Hyperlinks should require a configured modifier and pass through a URI
scheme policy before Windows opens them.

Search is the one missing engine boundary. Ghostty has a mature internal search
worker, but the current `libghostty-vt` C API does not expose it. Add or upstream
a small search API that returns tracked grid ranges, then wrap it safely in
Rust. Do not scrape the rendered viewport: it omits scrollback, mishandles
wrapped lines, and loses stable coordinates. The query editor and result
navigation belong in a GPUI overlay; the search itself belongs in Ghostty.

### Important cases

- wide glyphs, combining characters, wrapped lines, and rectangular selection;
- selection while output extends or trims scrollback;
- alternate-screen applications and mouse capture;
- wheel input over an unfocused pane;
- bracketed-paste terminators embedded in clipboard text;
- URI schemes other than `http`, `https`, and `file`;
- search cancellation and continued terminal output.

### Done when

Keyboard and mouse interaction work across normal and alternate screens;
selection remains stable while output arrives; paste cannot silently inject
unsafe input; and search covers the full scrollback without blocking rendering.

### Reference code

- Ghostty:
  [`Surface.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/Surface.zig),
  [`SelectionGesture.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/terminal/SelectionGesture.zig),
  [`search/Thread.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/terminal/search/Thread.zig),
  [`paste.h`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/include/ghostty/vt/paste.h).
- WezTerm:
  [`selection.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/wezterm-gui/src/selection.rs),
  [`mouseevent.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/wezterm-gui/src/termwindow/mouseevent.rs),
  [`overlay/copy.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/wezterm-gui/src/overlay/copy.rs).
- Kitty:
  [`window.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/window.py),
  [`screen.c`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/screen.c).

## 2. Add profiles and settings

### Implementation

Create a typed configuration module whose public entry point loads, validates,
and resolves settings into immutable runtime values. Keep these concepts
separate:

- `AppSettings`: window, sidebar, update, and quick-terminal behavior;
- `TerminalSettings`: font, colors, cursor, scrollback, and terminal policies;
- `LaunchProfile`: executable, arguments, starting directory, environment, and
  icon or label;
- `KeyBinding`: chord plus a typed action;
- derived `TerminalConfig` and `LaunchSpec` values used to create a pane.

Profiles need stable IDs so workspace files survive display-name changes.
Discovery for PowerShell, Command Prompt, WSL distributions, and Git Bash
should produce the same `LaunchProfile` type as user-defined entries. Merge
discovered and explicit profiles by stable ID, with explicit values taking
precedence.

Parse and validate a new settings file fully before publishing it. On reload,
keep the previous valid settings active and show actionable diagnostics for the
new invalid file. Existing panes keep the settings they were launched with
unless a field is explicitly safe to apply live; new panes use the new resolved
settings.

Keep definitions centralized. Defaults, validation, documentation, and any
future settings UI must not maintain four independent copies of a setting.
Kitty's generated configuration schema demonstrates the value of this, although
mightty only needs a small Rust-native model rather than a generator initially.
WezTerm's immutable configuration handle and generation counter are useful for
safe live reload without adopting its Lua runtime.

### Important cases

- missing executables and invalid starting directories;
- duplicate profile IDs and environment keys;
- WSL distributions being added or removed;
- fonts or themes unavailable after a settings change;
- malformed reloads while terminals are running;
- portable installs versus per-user configuration directories.

### Done when

Every new tab or split is launched from a resolved profile; settings failures
are visible and never partially applied; and key bindings, themes, and profile
selection no longer require source changes.

### Reference code

- Ghostty:
  [`Config.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/config/Config.zig).
- WezTerm:
  [`config/src/config.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/config/src/config.rs),
  [`config/src/lib.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/config/src/lib.rs).
- Kitty:
  [`options/definition.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/options/definition.py),
  [`conf/generate.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/conf/generate.py).

## 3. Introduce typed actions and a command palette

### Implementation

Replace the growing set of local GPUI action structs and hard-coded bindings
with one domain action enum. Variants can carry data where needed, for example:

```rust
enum AppAction {
    NewTab { profile_id: Option<ProfileId> },
    Split { direction: SplitDirection, profile_id: Option<ProfileId> },
    FocusPane(Direction),
    ResizePane { direction: Direction, amount: u16 },
    Copy,
    Paste,
    Search,
    ToggleQuickTerminal,
}
```

Associate actions with descriptors containing a stable ID, title, category,
default binding, and an availability predicate. GPUI bindings should be thin
adapters that dispatch an `AppAction`. Menus, the command palette, settings,
and future activation IPC consume the same descriptors and action values.

Build the palette as a normal GPUI overlay with fuzzy filtering and
context-sensitive availability. Profile-specific actions such as “New tab:
PowerShell” are generated from resolved profiles. Do not launch a terminal TUI
to host the palette; that is a useful Kitty process boundary but does not fit a
native GPUI application.

### Important cases

- shortcut conflicts and invalid action payloads in settings;
- actions unavailable without a selection or with only one pane;
- palette focus not leaking keystrokes into the PTY;
- generated profile actions updating after config reload;
- menu text and displayed key bindings staying synchronized.

### Done when

Adding an action requires one handler and one descriptor, and that action can be
invoked consistently from a shortcut, menu, or palette without duplicate
dispatch logic.

### Reference code

- WezTerm:
  [`keyassignment.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/config/src/keyassignment.rs),
  [`commands.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/wezterm-gui/src/commands.rs),
  [`overlay/launcher.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/wezterm-gui/src/overlay/launcher.rs).
- Ghostty:
  [`Binding.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/input/Binding.zig),
  [`command_palette.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/apprt/gtk/class/command_palette.zig).
- Kitty:
  [`actions.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/actions.py),
  [`command_palette/main.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kittens/command_palette/main.py).

## 4. Finish pane and workspace management

### Implementation

Change `SplitNode` from variable-length equal children to a binary layout tree:

```text
Leaf(PaneId)
Branch {
    axis,
    ratio,
    first,
    second,
}
```

This directly represents every divider and its ratio. A pure layout function
should transform the tree plus available bounds into positioned pane and
divider rectangles. Rendering and pointer hit-testing then consume that result.
Dragging a divider updates its ratio while enforcing a minimum number of
terminal rows or columns.

Directional focus should use rendered pane geometry: choose a pane in the
requested half-plane, then rank overlap and distance. This behaves predictably
even in nested asymmetric splits. Zoom is transient view state that renders one
leaf into the tab bounds; it must not rewrite the split tree.

Define a serializable `WorkspaceLayout` containing stable tab IDs, split axes
and ratios, active leaves, profile IDs, and trusted local working directories.
It must never contain a GPUI `Entity`, `Terminal`, PTY handle, or process ID.
Restoring a workspace means constructing fresh panes from launch specs and then
attaching them to the saved topology.

Start with one split layout model rather than Kitty's pluggable layout family.
WezTerm's binary tree already supports the operations mightty needs without a
layout-plugin abstraction.

### Important cases

- resize clamps at minimum pane dimensions;
- closing a leaf collapses its parent without changing unrelated ratios;
- directional focus across nested branches;
- zoom followed by close, split, or workspace restore;
- displays and window sizes differing from the saved session;
- missing profiles or invalid directories during restore.

### Done when

Dividers are draggable, pane focus and resize work in all directions, zoom is
lossless, and a saved multi-tab layout restores deterministically with fresh
processes.

### Reference code

- WezTerm:
  [`mux/src/tab.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/mux/src/tab.rs).
- Ghostty:
  [`SplitTree.swift`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/macos/Sources/Features/Splits/SplitTree.swift).
- Kitty:
  [`tabs.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/tabs.py),
  [`layout/base.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/layout/base.py),
  [`session.py`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/session.py).

## 5. Add shell integration and semantic commands

### Implementation

First expose the title, current working directory, bell, and semantic prompt
data already parsed by Ghostty. Use these to update tab labels, launch a new
pane in the active pane's reported local directory, jump between prompts, and
select or copy the preceding command output.

Ship small shell-integration resources that emit OSC 7 for the working
directory and OSC 133 markers for prompt, input, and output boundaries. Windows
is primary, so PowerShell integration is a first-class requirement, not a later
port. It should hook `prompt` and PSReadLine conservatively and preserve a
user's existing functions. POSIX scripts can follow the same capability model
for the Unix bridge.

Treat terminal-reported paths as untrusted metadata. Only a valid local `file`
URI for the local host can become a process working directory. Remote paths
remain display metadata. Likewise, a bell or semantic completion marker may
request a notification, but application focus and user policy determine whether
one is shown.

Do not infer command boundaries by reading screen text when integration is
absent. Semantic actions should be unavailable with a clear explanation until
the shell reports the required markers.

### Important cases

- nested shells, SSH sessions, and elevated PowerShell;
- prompts spanning multiple rows;
- commands producing no output or extremely long output;
- OSC data containing invalid UTF-8 or malicious paths;
- renamed or removed working directories;
- integration loaded more than once.

### Done when

Tabs track shell titles and working directories, new panes can inherit a trusted
local directory, and prompt/output actions operate on Ghostty semantic regions
without parsing rendered text.

### Reference code

- Ghostty:
  [`shell-integration/bash/ghostty.bash`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/shell-integration/bash/ghostty.bash),
  [`page.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/terminal/page.zig).
- WezTerm:
  [`performer.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/term/src/terminalstate/performer.rs),
  [`wezterm.sh`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/assets/shell-integration/wezterm.sh).
- Kitty:
  [`shell-integration/bash/kitty.bash`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/shell-integration/bash/kitty.bash).

## 6. Build the Windows product experience

### Implementation

Create a `QuickTerminalController` at the application layer. It owns one
persistent terminal window or workspace and controls:

- a configurable system-wide hotkey;
- show, hide, and toggle behavior;
- placement on the active monitor;
- topmost and frameless window state;
- size relative to the monitor work area;
- animation and optional hide-on-focus-loss.

The controller should reuse the normal pane, action, and profile types. It is a
special window lifecycle, not a second terminal implementation. Register the
global hotkey with Windows and use single-instance activation IPC so a second
process can ask the existing instance to toggle or open a profile.

Treat shipping as part of the feature: install per-user without administrative
rights, register uninstall and protocol metadata, sign artifacts, publish an
updater manifest, and test upgrades without losing settings or workspaces.
Default-terminal integration is a separate Windows contract layered on top of a
stable packaged application; it should not be coupled to ConPTY internals.
WezTerm's installer is a useful inventory of Windows integration points such as
architecture checks, minimum OS version, icons, uninstall metadata, and
Explorer “open here” commands, even though mightty should own its packaging
decisions.

### Important cases

- hotkey already registered by another application;
- monitor unplug, DPI change, taskbar work-area change, and remote desktop;
- focus moving to a child popup or palette;
- activation requests arriving during startup or shutdown;
- update rollback and a running terminal during upgrade;
- packaged and portable paths.

### Done when

One hotkey reliably toggles one warm terminal on the active monitor, repeated
launches activate the existing process, and signed installs and upgrades
preserve user data.

### Reference code

- Ghostty:
  [`QuickTerminalController.swift`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/macos/Sources/Features/QuickTerminal/QuickTerminalController.swift).
- WezTerm:
  [`windows-installer.iss`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/ci/windows-installer.iss).
- Kitty:
  [`quick_access_terminal/main.go`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kittens/quick_access_terminal/main.go).

## 7. Render Kitty graphics

### Implementation

Ghostty already parses the Kitty graphics protocol and exposes image and
placement data through its public C API. Add a safe lending wrapper in
`src/ghostty/graphics.rs`; borrowed graphics handles must not outlive the
terminal generation that produced them.

Keep shared image pixels separate from lightweight placements. Cache uploaded
GPUI/GPU image resources by Ghostty image identity and generation. Each render
snapshot yields placement records with source rectangle, destination geometry,
and z-layer. Draw negative-z placements below text and positive-z placements
above text, preserving crop, scroll, resize, and deletion behavior.

Do not parse graphics escape sequences or maintain protocol storage in the
renderer. Also do not enable file or shared-memory transfer media until Windows
path and process-boundary policies are explicit; terminal-controlled file reads
are a security boundary.

### Important cases

- the same image with multiple placements;
- image mutation reusing an ID with a new generation;
- source crop, cell-relative offsets, and fractional cell sizes;
- scrollback and alternate-screen transitions;
- placement deletion and GPU cache eviction;
- large images and storage limits;
- z-order relative to selection, cursor, and text.

### Done when

Known Kitty graphics fixtures render with correct crop, placement, z-order, and
deletion behavior while repeated frames reuse GPU resources and terminal output
cannot read arbitrary local files.

### Reference code

- Ghostty:
  [`kitty_graphics.h`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/include/ghostty/vt/kitty_graphics.h),
  [`graphics.zig`](https://github.com/ghostty-org/ghostty/blob/4c725242b7dbe8c77c6e227ef1f9540c5ef17921/src/terminal/kitty/graphics.zig).
- WezTerm:
  [`image.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/wezterm-cell/src/image.rs),
  [`terminalstate/kitty.rs`](https://github.com/wezterm/wezterm/blob/76b606ec597a3c0263fa60321548637451c0a547/term/src/terminalstate/kitty.rs).
- Kitty:
  [`graphics.c`](https://github.com/kovidgoyal/kitty/blob/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20/kitty/graphics.c).

## Recommended implementation sequence

Keep each item below as its own reviewable task:

1. Wrap Ghostty viewport, selection, mouse, paste, metadata, and callback APIs.
2. Add GPUI selection, scrolling, copy/paste, hyperlinks, and scrollbar UI.
3. Expose Ghostty search and add the in-pane search overlay.
4. Introduce typed settings, launch profiles, and safe config reload.
5. Replace hard-coded bindings with typed actions, then add the palette.
6. Convert the split model to a binary ratio tree; add focus, resize, and zoom.
7. Add serializable workspace layouts and restore with fresh processes.
8. Wire title, working-directory, bell, and semantic prompt data.
9. Add PowerShell and POSIX shell-integration resources and semantic actions.
10. Build single-instance activation and the Windows quick-terminal controller.
11. Establish signed installation and updates, then default-terminal support.
12. Add Ghostty graphics wrappers, GPU placement rendering, and fixture tests.

Interaction and settings should land before workspace persistence so the saved
model has stable profile and action vocabulary. Shell integration should land
before quick-terminal polish because directory inheritance and meaningful tab
titles affect that experience. Graphics comes last because it adds GPU resource
lifetime and security work without unblocking the core terminal workflow.

## Verification strategy

- Unit-test safe Ghostty wrappers, especially borrowed lifetimes, coordinate
  conversion, selection formatting, paste encoding, and graphics generations.
- Use golden feedback captures for wide glyphs, wrapped selection, search
  highlighting, semantic regions, and graphics placement.
- Black-box Windows tests should drive a real ConPTY for title, directory,
  OSC 133, bracketed paste, resize, process exit, and shutdown behavior.
- Exercise high-output and scrollback trimming while selecting or searching.
- Test workspace serialization independently from GPUI and PTY construction.
- Test quick-terminal placement on mixed-DPI multi-monitor setups and after
  monitor topology changes.

## Research baseline

The recommendations above were checked against these exact source revisions on
2026-07-24 so links remain stable:

| Project | Revision |
| --- | --- |
| Ghostty | [`4c725242b7dbe8c77c6e227ef1f9540c5ef17921`](https://github.com/ghostty-org/ghostty/tree/4c725242b7dbe8c77c6e227ef1f9540c5ef17921) |
| Kitty | [`4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20`](https://github.com/kovidgoyal/kitty/tree/4f2f6a076b2baa9310d54cbf4d8191f7dbaffd20) |
| WezTerm | [`76b606ec597a3c0263fa60321548637451c0a547`](https://github.com/wezterm/wezterm/tree/76b606ec597a3c0263fa60321548637451c0a547) |
