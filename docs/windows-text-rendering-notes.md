# Windows Text Rendering Notes

## Context

This note captures the current findings from debugging terminal text rendering in `mightty` on Windows.

The immediate user-visible issue was:

- bold prompt segments like `v0.1.0` after emoji/icons were difficult to see
- at times text after `📦` disappeared entirely in the GPUI-rendered output

## What Ghostty Returns

`libghostty-vt` is not the source of the problem.

From the feedback captures:

- `capture.json` consistently shows the correct terminal row text
- cells after `📦` exist at the expected columns
- those cells are marked `bold=true`
- Ghostty color/style state is internally consistent

So the Ghostty VT/parser/buffer side is behaving correctly for this case.

## Current Render Pipeline

The path in `mightty` is:

1. shell output bytes go into Ghostty via `Terminal::vt_write`
2. Ghostty render state is read through the manual FFI wrapper in `src/ghostty/mod.rs`
3. `src/widget/mod.rs` converts Ghostty cells into per-row GPUI text segments
4. GPUI shapes/draws those segments through its Windows DirectWrite backend

Important implication:

- Ghostty returns semantic cells
- GPUI/DirectWrite turns those cells into pixels
- the visible bug happens in the render layer, not in Ghostty

## What The Captures Proved

The paired capture system (`capture.json` + `capture.png`) was critical.

Repeated pattern:

- JSON said `📦 v0.1.0` was present and bold
- PNG showed `v0.1.0` missing or too dim to read

That means:

- terminal state was correct
- visible output was wrong

This narrowed the failure to GPUI/Windows text rendering behavior.

## GPUI / Windows Findings

Research into GPUI and its Windows backend showed:

- GPUI shapes text through a single DirectWrite `TextLayout` per text object
- font fallback and text shaping are handled by GPUI/DirectWrite, not by Ghostty
- on Windows, GPUI explicit font fallback handling is tied to the system font collection
- embedded custom fonts are not equivalent to a full terminal-oriented fallback strategy

In practice, mixed lines containing:

- monospace text
- Nerd Font symbols
- emoji fallback
- bold/intense terminal attributes

are fragile in this stack.

## Why Windows Terminal Looks Better

Windows Terminal is not relying on the same behavior.

Relevant documented behavior:

- Windows Terminal has `intenseTextStyle`
- default behavior is `bright`, not purely `bold`
- Windows Terminal also has `adjustIndistinguishableColors`

That means Windows Terminal often treats `\x1b[1m` as:

- brighter text color
- optionally adjusted for visibility

instead of depending only on a heavier font face.

So if `v0.1.0` looks fine in Windows Terminal, that does **not** mean the font itself is enough. It likely means Windows Terminal is applying terminal-specific intensity and contrast rules that `mightty` does not yet mirror.

Sources used while investigating:

- <https://learn.microsoft.com/en-us/windows/terminal/customize-settings/profile-appearance>
- <https://github.com/microsoft/terminal>

## Working Hypothesis

The remaining bold-visibility problem is best understood as:

- not a Ghostty bug
- not primarily a Unicode-width bug
- not just "font weight failed"

It is a terminal display-policy issue combined with GPUI/DirectWrite behavior.

More concretely:

- Ghostty emits a colored bold/intense prompt
- GPUI/DirectWrite does not make that bold text visually distinct enough on Windows in this setup
- preserving the original terminal color too literally can make "bold" unreadable

## Changes Tried So Far

Several app-side experiments were made in `src/widget/mod.rs`:

- rendering rows as multiple segments instead of one giant styled line
- isolating non-ASCII / wide-cell segments
- switching between `IBM Plex Mono` and `JetBrainsMono Nerd Font Mono`
- making bold text use brighter foregrounds
- trying stronger weight values like `EXTRA_BOLD`
- experimenting with synthetic background emphasis
- reverting the global background override when it proved misleading
- moving toward a brighter display palette for bold text

Results:

- segmenting rows helped isolate some emoji-related layout/pathology
- font family changes materially changed symptoms
- pure weight changes were not reliable enough
- subtle color tweaks were too weak

## Practical Direction

If we want `mightty` to behave more like Windows Terminal, the most promising direction is:

1. treat terminal bold/intense as a display policy, not just a font-weight request
2. map bold/intense colors to a brighter display palette when needed
3. optionally still request heavier weight, but do not rely on it for readability
4. keep using the feedback capture system to compare semantic state (`json`) vs actual output (`png`)

In short:

- terminal "bold" on Windows should likely mean "bright and maybe heavier"
- not "same color, ask DirectWrite for a bolder face, hope for the best"

## Open Follow-Up

Still worth investigating later:

- whether GPUI can be given a better explicit font/fallback setup for terminal rows
- whether emoji/symbol/text should be rendered with separate font-family policies
- whether terminal ANSI intense handling should be modeled explicitly in the renderer
- whether the feedback JSON should also record the final display colors after renderer policy is applied
