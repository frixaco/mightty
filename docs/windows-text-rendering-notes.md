# Windows Text Rendering Notes

## Purpose

This note records Windows text-rendering constraints in `mightty` and the
renderer policy for mixed terminal text.

## Render Path

Terminal text follows this path:

1. shell output bytes are written into Ghostty with `Terminal::vt_write`
2. Ghostty render state is read through the project-owned safe module in `src/ghostty/`
3. `src/widget/render.rs` converts Ghostty cells into positioned GPUI text segments
4. GPUI and DirectWrite shape those segments into pixels on Windows

Ghostty owns terminal parsing, cell state, Unicode width, and style state.
GPUI/DirectWrite owns the final font fallback and pixel output.

## Feedback Capture Contract

Every feedback capture writes a semantic `capture.json`. On Windows it also
attempts to write a visual `capture.png` of the window client area; a PNG
failure is reported without discarding the JSON. Non-Windows captures are JSON
only.

When both files exist, they compare terminal state with rendered pixels:

- JSON records row text, cell columns, style flags, and terminal colors.
- PNG records the final GPUI/DirectWrite output.
- Differences between JSON state and PNG pixels point to app-side rendering,
  font fallback, shaping, or color policy.

## Renderer Policy

`src/widget/render.rs` treats terminal bold/intense text as display policy, not
only as a heavier font request.

Behavior:

- terminal rows are rendered as positioned segments, not one large row string
- non-ASCII and multi-column cells are isolated into their own layout segments
- terminal font features disable ligatures, contextual alternates, and kerning
- the text style includes a broad monospace, CJK, and emoji fallback list
- bold cells are mapped through a brighter display palette
- bold foreground colors are further adjusted when contrast against the
  effective background is too low
- the renderer still requests `FontWeight::BOLD`, but readability does not rely
  on DirectWrite making the heavier face visually distinct

This is intentionally closer to terminal behavior such as Windows Terminal's
`intenseTextStyle=bright` than to plain GUI text styling.

## Windows-Specific Constraints

Mixed terminal rows can combine:

- monospace ASCII text
- Nerd Font symbols
- emoji fallback
- CJK fallback
- bold/intense ANSI attributes
- foreground and background colors controlled by terminal state

That combination is fragile when a GUI text stack handles fallback and shaping
without terminal-specific intensity and contrast rules.

Windows Terminal also has display policies such as `intenseTextStyle` and
`adjustIndistinguishableColors`, so matching Windows Terminal output requires
more than matching the font family.

Useful references:

- <https://learn.microsoft.com/en-us/windows/terminal/customize-settings/profile-appearance>
- <https://github.com/microsoft/terminal>

## Follow-Ups

Still worth investigating:

- whether GPUI can be given a more explicit terminal-oriented fallback setup
- whether emoji, symbols, CJK, and text should use separate font-family policies
- whether feedback JSON should include final display colors after renderer policy
- whether the bold/intense policy should become configurable when user settings exist
