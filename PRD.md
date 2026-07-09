# UI Tabs and Pane Management

## Goal

Define mightty's lightweight tab management, pane splitting, pane closing, and pane chrome behavior.

## Scope

- Add tabs controlled by keyboard shortcuts.
- Render tabs as a persistent vertical list in a left sidebar.
- Allow hiding and showing the sidebar with `Ctrl+B`.
- Support splitting the active pane downward.
- Allow closing the active pane or active tab with `Ctrl+D`.
- Add subtle rounded corners to tabs and terminal panes.
- Draw green separator lines between panes.

## User-Facing Requirements

### Tabs

- `Ctrl+T` creates a new tab.
- Tabs are listed in a persistent left sidebar, not in the title bar.
- Each tab shows its numeric slot, `1` through `9`, so it can be selected with `Ctrl+1` through `Ctrl+9`.
- Each tab also shows a short title derived from the tab's active terminal context, such as a shortened home-directory-relative path when available.
- Tab items should be compact and arranged as a vertical list.
- Tab items should have slightly rounded corners, about `4px`.
- A newly created tab becomes the active tab.
- Clicking a tab in the sidebar should switch to that tab.
- `Ctrl+1` through `Ctrl+9` switch to the matching tab when it exists.
- The visible tab number is based on tab order and should renumber after tabs close.
- The running app keeps at least one tab open; closing the final remaining pane exits the app.

### Sidebar

- The left sidebar should be persistent whenever the main window is open.
- The sidebar should use the same black background as the rest of the window.
- The sidebar width is fixed at `160px` so each tab can show its number and short title without looking like a square icon.
- `Ctrl+B` toggles the sidebar between visible and hidden.
- When hidden, the sidebar should completely disappear and the terminal area should reclaim the full width.
- Hiding the sidebar must not lose tab state, active tab state, pane state, or terminal state.
- The sidebar should not interfere with the custom title bar or Windows window control buttons.
- The active tab should be visually distinct without introducing a heavy or colorful theme.
- Active tab styling should use a subtle dark-gray fill or border. Green should remain reserved for pane separators.

### Pane Splitting

- Existing `Alt+Enter` behavior should continue to create a pane to the right.
- `Alt+Shift+Enter` creates a pane under the active pane.
- New splits should start at a fixed `50/50` size.
- The new pane should become active after creation.
- A green separator line should be drawn between panes.

### Pane Styling

- Each terminal pane should have a slight rounded corner, about `4px`.
- Rounded pane corners should not create visible gray gaps; the surrounding background should remain black.
- Terminal rendering and resize behavior should continue to use the pane's actual local bounds.

### Closing

- `Ctrl+D` closes the active pane if the active tab contains multiple panes.
- `Ctrl+D` closes the active tab if the active tab contains exactly one pane.
- `Ctrl+D` is owned by mightty for pane and tab closing and should not be sent to the shell.
- If there is only one tab with one pane, `Ctrl+D` exits the app.
- After closing a pane, focus should move to a sensible remaining pane.
- After closing a tab, focus should move to the previous tab when possible, otherwise the next tab.

## Implementation Notes

- `PaneContainer` owns tabs, sidebar state, top-level actions, and key bindings.
- Each tab owns one root `Split`.
- `Split` owns `Entity<TerminalWidget>` leaves directly; there is no separate `Pane` wrapper.
- `SplitDirection::Row` creates right splits and `SplitDirection::Column` creates downward splits.
- Exited terminal panes signal `PaneContainer` through a GPUI task and are removed only when the tab still has another pane.
- Tab titles use the app process current directory with `~` as a home-directory fallback.

## Verification Checklist

- New tabs render and switch correctly.
- `Ctrl+B` hides and restores the sidebar without losing tab or pane state.
- `Ctrl+1` through `Ctrl+9` switch to existing tabs.
- Right and downward splits resize correctly.
- Focus follows new panes and survives close operations.
- `Ctrl+D` removes a pane, removes a tab, or exits the app according to the active pane/tab count.
- Run:
  - `cargo fmt`
  - `cargo check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`

## Non-Goals

- No draggable tab reordering.
- No tab close buttons.
- No titlebar tab strip.
- No sidebar animation.
- No mouse-based pane splitting.
- No horizontal/vertical split resizing handles beyond the requested separator line.
- No persisted session state.
- No user-configurable key bindings.

## Risks and Open Questions

- Active pane tracking remains important because `Alt+Shift+Enter` and `Ctrl+D` affect the intended pane.
- Tab title derivation uses a stable app-current-directory fallback.
- Pane removal must keep nested splits normalized so empty containers do not remain.
- Sidebar layout must preserve terminal resize correctness so each terminal still receives its actual local bounds.
- Title bar hit regions must remain simple; Windows control buttons and drag behavior must remain intact.
