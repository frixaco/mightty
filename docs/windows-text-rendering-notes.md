# Windows Text Rendering Notes

## Purpose

This note records the current Windows text-rendering constraints in `mightty` and
the renderer policy that exists to work around them.

The original user-visible problem was that bold prompt segments after mixed
emoji/Nerd Font content, such as `📦 v0.1.0`, could become too dim or disappear
in the GPUI-rendered output even though the terminal state was correct.

## Render Path

Terminal text follows this path:

1. shell output bytes are written into Ghostty with `Terminal::vt_write`
2. Ghostty render state is read through the `libghostty-vt` facade in `src/ghostty/mod.rs`
3. `src/widget/mod.rs` converts Ghostty cells into positioned GPUI text segments
4. GPUI and DirectWrite shape those segments into pixels on Windows

Ghostty owns terminal parsing, cell state, Unicode width, and style state.
GPUI/DirectWrite owns the final font fallback and pixel output.

## What The Captures Proved

Feedback captures pair a semantic `capture.json` with a visual `capture.png`.
For the prompt rendering issue, the captures showed:

- JSON contained the expected row text
- cells after `📦` existed at the expected columns
- those cells were marked `bold=true`
- PNG output was missing or visually muting that same text

That ruled out Ghostty's VT/parser/buffer state for this case and narrowed the
problem to the app-side rendering path.

## Current Renderer Policy

`src/widget/mod.rs` now treats terminal bold/intense text as display policy, not
only as a heavier font request.

Current behavior:

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
