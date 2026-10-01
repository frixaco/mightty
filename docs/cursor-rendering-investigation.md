# Cursor rendering investigation

Investigated and fixed October 1, 2026. The findings and original line numbers
below describe the pre-fix working tree. Existing uncommitted scheduling and
painting work was preserved. The implemented presentation boundary is described
in [terminal-presentation-design.md](terminal-presentation-design.md).

## Findings

### 1. Hidden terminal cursors are painted (confirmed primary cause)

`src/widget/render.rs:478` decides cursor visibility from pane focus and Mightty's
own blink phase. It never checks the terminal's cursor visibility. A valid
`Snapshot::cursor_viewport()` result is treated as permission to paint.

Those are independent properties. Ghostty retains cursor coordinates while
DECTCEM (`CSI ? 25 l`) hides its appearance. Its render snapshot exposes
`CURSOR_VISIBLE`, `CURSOR_BLINKING`, and `CURSOR_VISUAL_STYLE` separately from
`CURSOR_VIEWPORT_HAS_VALUE`. Our safe snapshot wrapper exposes coordinates but
none of those appearance fields (`src/ghostty/render.rs:99`). Ghostty computes
visibility at `ghostty/src/terminal/render.zig:425` and viewport position at
line 558 independently. The C getters preserve that distinction
(`ghostty/src/terminal/c/render.zig:345`).

A TUI's terminal cursor is also its output insertion position: moving it to
write a label or animate an icon does not mean the app wants a visible input
caret there. Mightty makes those hidden output positions visible. The offending
visibility expression also exists in HEAD, before the current uncommitted
direct-painting changes.

### 2. Synchronized output does not suppress rendering (confirmed contributor)

Amp uses `CSI ? 2026 h` / `CSI ? 2026 l` to bracket updates. The parser recognizes
the mode, but Mightty does not consult it. `start_output_task` applies bounded
batches, not complete TUI frames, notifies GPUI, then yields for 1 ms
(`src/widget/mod.rs:731`). The painter unconditionally updates its snapshot
(`src/widget/render.rs:127`). PTY read boundaries and processing budgets can
therefore expose a partially completed update, including an intermediate cursor
position. Blink notifications provide another route into the same painter.

The library's render-state update is a state copy, not Ghostty's complete
application renderer. Ghostty implements the presentation guard outside the
library: `ghostty/src/renderer/generic.zig:1197` returns without updating the
frame when synchronized output is active. Its I/O thread also provides a reset
timer (`ghostty/src/termio/Thread.zig:370`). Neither mechanism is supplied by our
`RenderState::update` call. Simply increasing the batching budget cannot establish
frame boundaries and would undermine the existing UI responsiveness work.

### 3. Application cursor shape and blink requests are ignored (related defect)

The painter selects `self.config.cursor_style` and
`self.config.cursor_blink`, rather than Ghostty's resolved snapshot appearance.
`schedule_cursor_blink` likewise follows only configuration
(`src/widget/mod.rs:702`). Consequently DECSCUSR (`CSI Ps SP q`) and DEC mode 12
cannot change the drawn cursor's shape or blinking, even when the parser accepts
them. This does not by itself explain the wandering coordinates.

Mightty also sets default cursor color but does not initialize Ghostty's default
cursor shape/blink from its configuration (`src/widget/mod.rs:366`). A fix that
starts using snapshot appearance must initialize those defaults too, so normal
shells retain the user's configured cursor and application reset sequences
restore it correctly.

## Runtime evidence

Installed Amp: `0.0.1790856055-g589fab`, released October 1, 2026. Launched with
no user prompt, in a disposable directory, through the existing `PtyParts`
ConPTY bridge. Used a 120-column, 30-row Ghostty terminal, installed its PTY reply
callback, and observed each received output chunk for approximately eight
seconds. Closed only this probe's child process afterward.

| Observation | Result |
| --- | ---: |
| Received chunks sampled | 672 |
| Samples with hidden cursor and valid viewport coordinates | 650 |
| Distinct coordinates among those hidden samples | 41 |
| Captured output bytes, including shutdown drain | 1,455,755 |
| Cursor-hide sequences (`CSI ? 25 l`) | 5 |
| Cursor-show sequences (`CSI ? 25 h`) | 4 |
| Synchronized-update starts | 269 |
| Synchronized-update ends | 269 |

Examples of hidden coordinates were `(71, 9)` through `(78, 9)`, numerous
positions around rows 19-22, and columns 111-119 on row 29. Coordinates are
zero-based. These are read-boundary observations, not counts of displayed GPU
frames. The capture proves Amp's hide and synchronization commands reach our
parser through ConPTY; neither command is completely lost by the bridge.

A separate temporary GPUI test exercised the actual terminal widget painter
with blinking disabled to make the result deterministic:

| Update | Ghostty visibility | Cursor coordinates | Painter trace |
| --- | --- | --- | --- |
| Input position | visible | `(3, 11)` | `cursor count=1` |
| Hide cursor, write label | hidden | `(11, 1)` | `cursor count=1` |
| Write spinner while hidden | hidden | `(25, 4)` | `cursor count=1` |
| Begin sync, move away from input | visible | `(19, 7)` | `cursor count=1` |
| Restore input position, end sync | visible | `(3, 11)` | `cursor count=1` |

The temporary test passed assertions for the original incorrect behavior. It demonstrated
both painting a hidden cursor and preparing a cursor at an unfinished update's
position. It did not measure physical display timing. No full-GUI Amp recording
or original-session trace was taken, so the contribution of each defect to every
individual flash in the reported session is not quantified.

The temporary test and example were removed from source after verification.
Probe sources and raw output remain locally under `target/cursor-investigation/`
(`widget_probe.rs`, `amp_probe.rs`, `amp.vt`). They are diagnostic artifacts, not
new production APIs. Formatting check passed after cleanup.

## Smallest corrective scope

1. Expose snapshot cursor visibility, shape, and blinking through the existing
   safe Ghostty wrapper. Initialize Ghostty's cursor defaults from configuration.
   Use the resolved snapshot state to decide appearance; viewport coordinates
   alone must never imply visibility. Keep the existing focus and blink-phase
   policy around those terminal properties.
2. While mode 2026 is active, continue parsing output and serving input but retain
   the previous complete presentation. Handle completion, timeout, and resize
   explicitly. Suppressing only output notifications is insufficient because
   other events can trigger the painter. Preserve coherence between text, cursor,
   graphics, and presented-frame capture metadata.
3. Leave regression checks for hidden cursor movement, show/restore, application
   steady/blinking shape requests, and synchronization split across PTY reads.

Do not pin the cursor to a guessed input-box location: terminal emulators cannot
infer TUI widget semantics, and that would break valid application cursor moves.

## Implemented fix and regression evidence

The safe wrapper now exposes resolved cursor visibility, shape and blinking and
initializes native defaults from settings. One owned `TerminalFrame` commits all
terminal-backed layers together. The GPUI painter, IME anchor and capture consume
that frame. Hidden cursor movement therefore has no cursor pixels, and holding
mode 2026 freezes text, selection, search highlights, cursor and graphics while
parsing, keyboard encoding, terminal effects and PTY replies continue.

The one-second recovery timer resets Ghostty's actual mode, with a generation
count obtained at native parser ground boundaries. An end/begin pair in one
chunk cannot be mistaken for one long hold. Resize, terminal reset and EOF also
release presentation. Build failure retains the old frame and retries without
further output. Live and offscreen painting keep separate records of the frame
and local appearance used for each element tree; capture never relabels old
pixels with a newer output/grid revision.

The retained regressions cover every byte split of the short sync fixture and
fixed/random chunking of an 80-update synthetic, sanitized TUI stream with
Unicode, styles and hyperlinks. They compare final cells/colors/cursor/viewport,
not only final text. Fake-clock checks cover timeout without further output,
back-to-back updates, obsolete deadlines, resize/reset/EOF, and an exhausted
graphics texture budget followed by successful retry. Held-frame checks include
selection, search, capture, input and replies. No raw Amp session was added to
source.

`cargo test native_terminal_presentation -- --ignored --test-threads=1` runs a real
Windows GPU window without a shell. It compares raster pixels for hide/show and
hidden movement, block glyph contrast, bar/underline/default/hollow shapes,
wide-head/wide-tail footprint, steady/blinking appearance, focus, alternate-buffer restore,
IME anchoring and held graphics replacement. This isolates the renderer from
ConPTY, which can consume Kitty APC commands. Existing selection, search, shaping,
composition and graphics fidelity checks remain in place.

`tools/control-smoke.ps1 -PresentationOnly` verifies ConPTY mode delivery,
retained presented capture versus live output, a next-frame timeout during the
hold, actual mode recovery without additional output, PowerShell backspace/text
editing and Neovim insert/save through normal GPUI input. It exposed an existing
shared pane-focus bug: `focus_active` resynchronized to the previously focused
pane after `focus_target` chose a new one. The shared function now focuses its
selected pane directly, with a regression. Global control discovery also treated
terminated processes with retained Windows handles as live because their creation
times remained readable. It now checks native exit status before probing a pipe;
a retained-child-handle regression covers that failure. No unrelated registry
files or processes were removed. Single- and three-pane Amp runs type
and erase characters without submitting a model prompt, resize the window and
drag the divider. Multi-producer stress also checks final output/EOF and close.

The [scheduling follow-up](terminal-output-scheduling-plan.md)
records current measurements, check commands and artifact locations. Native
composition is covered through the existing text-handler tests and candidate
geometry; the validation does not change the user's OS input-language settings.

## Protocol references

- [Microsoft's VT sequence documentation](https://learn.microsoft.com/en-us/windows/console/console-virtual-terminal-sequences)
  defines cursor visibility, positioning, and cursor shape/blink sequences.
- [Ghostty's synchronized output guidance](https://ghostty.org/docs/help/synchronized-output)
  explains why a terminal must wait for a completed update before presenting it.
- The pinned Ghostty checkout provides the implementation evidence cited above;
  its application renderer behavior is distinct from the embedded VT library.
