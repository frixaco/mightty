# mightty Renderer Migration PRD

## Goal

Fully migrate `mightty` to the `gpui-ghostty` rendering architecture with no fallback to the current segmented `div`/`StyledText` renderer.

The target renderer should treat the terminal viewport as a custom-rendered text surface: shape full lines, apply terminal style runs, cache layouts, and paint backgrounds, cursor, and terminal-specific graphics separately.

## Non-Goals

- Do not keep the current per-segment absolute-positioned GPUI element renderer as a compatibility path.
- Do not patch individual Windows font/color symptoms once the new renderer path is in place.
- Do not rewrite shell/process handling unless required by the wrapper/session migration.

## Migration Plan

### 1. Sync upstream libghostty first

Fetch upstream Ghostty/libghostty changes and pin the exact commit used by `mightty`.

Tasks:

- Update the local `ghostty` source tree to the desired upstream commit.
- Confirm the required Zig version for that commit.
- Keep `cargo build` producing `ghostty-vt.dll` cleanly on Windows.
- Ensure `build.rs` invokes the required Zig version deterministically.
- Verify `cargo run` works from a clean checkout.

Acceptance criteria:

- `cargo build` succeeds after cleaning the `mightty` build artifacts.
- `ghostty-vt.dll` is copied into the expected `target/debug` and `target/debug/deps` locations.
- The pinned Ghostty commit and Zig version are documented.

### 2. Replace the wrapper, not patch around it

Move from the current hand-written render-state cell iterator API toward the `gpui-ghostty` wrapper/session model.

Required capabilities:

- Viewport lines.
- Style runs per row.
- Cursor position.
- Default foreground/background colors.
- Palette and reverse-color dirty handling.
- OSC color responses.
- DSR cursor/status responses.

Acceptance criteria:

- The Rust wrapper matches the updated Ghostty C/Zig ABI.
- Existing terminal input/output still flows through the shell session.
- The old renderer is not expanded to support missing wrapper behavior.

### 3. Introduce the new terminal view surface

Replace per-cell/per-segment GPUI rendering with a custom GPUI `Element`.

The element should prepaint:

- Shaped full text lines.
- `TextRun`s per terminal style run.
- Background quads.
- Cursor quad.
- Box-drawing quads.
- Selection/marked text support if needed later.

Acceptance criteria:

- The terminal viewport is rendered by one custom surface, not many styled `div`s.
- Text layout is cached per line where possible.
- Cursor and backgrounds are painted independently of text shaping.

### 4. Remove the old segment renderer

Delete the current renderer-specific structures and logic once the new surface is functional.

Remove or replace:

- `RowSegment`.
- `RowTextStyle`.
- Absolute-positioned `StyledText` segment rendering.
- Fixed renderer assumptions tied to segment layout.
- Bold color hacks that become obsolete under the new style-run renderer.

Acceptance criteria:

- There is only one production renderer path.
- No old renderer fallback remains.
- Dead code from the segmented renderer is removed.

### 5. Port font and metrics model

Use GPUI font objects and measured terminal cell metrics rather than fixed cell dimensions.

Required behavior:

- Use `gpui::Font`.
- Disable terminal-unfriendly features such as `calt`, `liga`, and `kern`.
- Use explicit monospace and emoji fallbacks.
- Measure cell metrics through GPUI shaping.
- Resize the terminal from measured cell width/height.

Acceptance criteria:

- Cell width and height are derived from GPUI text shaping.
- Terminal resizing remains aligned with rendered content.
- Wide characters, emoji, and Nerd Font glyphs do not drift subsequent columns.

### 6. Port style behavior deliberately

Start by matching `gpui-ghostty` behavior, then decide where Windows-style intense text policy belongs.

Required style support:

- Bold.
- Italic.
- Faint.
- Underline.
- Strikethrough.
- Inverse.
- Foreground/background colors.
- Palette changes.

Open decision:

- Whether Windows-style bold/intense color adjustment should live in the wrapper/session layer, renderer layer, or a configurable display policy.

Acceptance criteria:

- ANSI style output matches terminal state.
- Bold visibility is handled intentionally, not by relying only on font weight.
- Reverse video and palette updates force correct redraws.

### 7. Rebuild feedback tests around the new renderer

Keep the existing semantic capture approach, but align it with the new renderer.

Test cases:

- Emoji before bold prompt text.
- ANSI bright and bold colors.
- Inverse text.
- Box drawing.
- Wide characters.
- PowerShell prompt segments.
- Palette updates.
- Cursor positioning.

Acceptance criteria:

- Captured terminal state and rendered output agree for the known Windows failure cases.
- Regression captures can distinguish parser/state bugs from render bugs.

### 8. Clean build and runtime assumptions

Make the development path reliable from a clean checkout.

Tasks:

- Pin the required Zig version.
- Document setup requirements.
- Avoid untracked generated build artifacts such as `ghostty/zig-pkg`.
- Keep `cargo run` working without manual environment overrides.

Acceptance criteria:

- A developer can run `cargo build` and `cargo run` after following documented setup.
- Build failures report actionable toolchain/version errors.
- Git status stays clean after a successful build, except for intentionally ignored build artifacts.

## First Milestone

The first milestone is upstream libghostty synchronization.

Deliverables:

- Updated local Ghostty source.
- Updated Rust/Zig wrapper if the ABI changed.
- Deterministic Zig version selection.
- Passing `cargo build`.
- Short documentation of the pinned Ghostty commit and Zig version.

Only after this milestone should the GPUI renderer migration begin.

Status:

- Ghostty pinned commit: `b0d359cbbd945f9f3807327526ef79fcaf0477df`.
- Ghostty commit date: 2026-04-23.
- Ghostty commit subject: `more zon2nix update for improved 0.16 compatibility (#12405)`.
- Required Zig version: `0.15.2`, read from `ghostty/build.zig.zon`.
- `cargo build` status: passing on Windows.
- `cargo run` smoke status: launches and stays running; the launched `mightty.exe` was stopped after verification.
- DLL output verified: `target/debug/ghostty-vt.dll` and `target/debug/deps/ghostty-vt.dll`.

## Execution Notes

Completed in this pass:

- Fast-forwarded the local `ghostty` source from `7127abfe285014c62bc1f9b24d4e038af7f94afa` to `b0d359cbbd945f9f3807327526ef79fcaf0477df`.
- Kept Zig pinned to `0.15.2` via `.mise.toml` and `build.rs` validation.
- Removed generated `ghostty/zig-pkg` output and confirmed builds no longer leave the nested Ghostty checkout dirty.
- Updated the Rust wrapper for upstream `ghostty_terminal_set` effects and default color APIs.
- Routed Ghostty write-PTY responses back to the shell input channel so DSR/OSC/query responses are not silently dropped.
- Moved default foreground/background/cursor/palette ownership into Ghostty terminal state.
- Fixed GPUI color construction from `rgba(0xRRGGBB)` to `rgb(0xRRGGBB)` so terminal colors are opaque instead of accidentally using blue as alpha.
- Tightened `.gitignore` root-only generated directory ignores so `src/ghostty/` is not hidden as ignored source.

Remaining next slice:

- Replace the current render-state cell iterator wrapper with viewport line/style-run APIs.
- Introduce the custom GPUI terminal surface.
- Delete `RowSegment`, `RowTextStyle`, absolute-positioned `StyledText` rendering, and bold color hacks after the custom surface is functional.
