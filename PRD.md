# UI Tabs and Pane Management PRD

## Goal

Add lightweight tab management, vertical pane splitting, pane closing, and pane chrome refinements to mightty while preserving the existing custom black title bar and terminal rendering behavior.

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
- The visible tab number is based on the current tab order and should renumber after tabs close.
- The app must always keep at least one tab open.

### Sidebar

- The left sidebar should be persistent whenever the main window is open.
- The sidebar should use the same black background as the rest of the window.
- The sidebar width should be fixed at `52px` for now.
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
- If there is only one tab with one pane, `Ctrl+D` should not leave the app empty. The preferred behavior is to ignore the shortcut in that state.
- After closing a pane, focus should move to a sensible remaining pane.
- After closing a tab, focus should move to the previous tab when possible, otherwise the next tab.

## Implementation Plan

1. Inspect the current `PaneContainer`, `Split`, and `Pane` ownership model.
   - Confirm how key bindings are dispatched.
   - Confirm how terminal focus and resize are currently tracked.
   - Decide whether `Split` should become orientation-aware directly or whether a split tree is needed.

2. Add a tab model above `Split`.
   - Each tab owns its own root split.
   - `PaneContainer` tracks the active tab index.
   - Numeric tab labels are derived from tab order.
   - Tab titles are derived from the active terminal context when available, with a stable fallback.

3. Add left sidebar rendering.
   - Render numeric tab items as a vertical list.
   - Render each tab with its number and a short title.
   - Use a fixed `52px` width while visible.
   - Add collapsed state so `Ctrl+B` can remove the sidebar from layout without destroying tab state.
   - Keep the sidebar background black.
   - Make tab items clickable.
   - Keep the title bar free of tab UI.
   - Preserve the right-side Windows control buttons and draggable title bar behavior.

4. Add key bindings.
   - `Ctrl+T` creates a tab.
   - `Ctrl+B` toggles sidebar visibility.
   - `Ctrl+1` through `Ctrl+9` switch tabs.
   - `Alt+Shift+Enter` splits the active pane downward.
   - `Ctrl+D` closes the active pane or tab according to pane count.

5. Extend split layout.
   - Preserve right splits for `Alt+Enter`.
   - Add downward splits for `Alt+Shift+Enter`.
   - Start new splits at a fixed `50/50` ratio.
   - Prefer a minimal orientation-aware split tree if needed:
     - `Pane(Entity<Pane>)`
     - `Split { direction, children }`

6. Add pane and separator styling.
   - Add `4px` pane rounding.
   - Draw green separator lines between panes.
   - Keep all background surfaces black.

7. Verify behavior.
   - New tabs render and switch correctly.
   - `Ctrl+B` hides and restores the sidebar without losing tab or pane state.
   - `Ctrl+1` through `Ctrl+9` switch to existing tabs.
   - Splits resize correctly.
   - Focus follows new panes and survives close operations.
   - Closing never leaves zero tabs or zero panes.
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

- Active pane tracking is the main implementation risk. The split structure needs a reliable active pane id so `Alt+Shift+Enter` and `Ctrl+D` affect the intended pane.
- Tab title derivation may be limited by what terminal or shell state is currently available. If the active directory cannot be detected reliably, use a simple stable fallback title.
- If `Split` becomes a tree, pane removal needs careful cleanup so nested splits do not leave empty containers.
- Sidebar layout must preserve terminal resize correctness so each terminal still receives its actual local bounds.
- Title bar hit regions must remain simple because tabs no longer live there; Windows control buttons and drag behavior should continue unchanged.
