# Terminal presentation design

Implemented October 1–2, 2026. The accepted contract below is enforced by
`src/widget/presentation.rs`; validation and limitations are recorded in the
[cursor investigation](cursor-rendering-investigation.md) and
[scheduling results](terminal-output-scheduling-plan.md).

## Decision

Rebuild the boundary between live terminal state and presentation. Keep Ghostty's
VT engine, GPUI, the shell bridges, and the application shell. The demonstrated
bugs are missing integration semantics, not evidence that those components need
replacement.

One terminal owner mutates Ghostty. One presentation builder produces a committed,
immutable terminal frame. Every pixel-producing path consumes that frame. Painting
must not read or mutate a live `Terminal` or update a `RenderState`.

Keep the current single-thread ownership while establishing this contract. A
worker migration is not required for cursor or synchronization correctness; it
would also require moving mode-dependent input encoding, selection, search, PTY
replies, and resize ordering. The existing performance investigation does not
justify that additional migration as part of this fix. Do not introduce channels
or unsafe Send/Sync implementations merely to anticipate it.

## Ownership and data flow

```text
PTY bytes / input / resize / viewport actions
                    |
                    v
          single terminal owner
          Ghostty + protocol state
                    |
             presentation gate
                    |
                    v
          immutable committed frame
                    |
         +----------+------------+
         |          |            |
      GPUI paint  IME anchor   frame capture
```

The committed frame contains rows and resolved cell styles, palette/default
colors, resolved cursor state, grid geometry, buffer/viewport identity, terminal
selection, graphics placements with owned image references, and the applied
output/geometry revision. It must own its data; lending FFI iterators and borrowed
image pixels cannot escape the builder. Reuse the existing owned graphics/cache
machinery and GPUI shaping. GPU resources remain on their appropriate owner.

The builder copies the needed data from one terminal state before subsequent
terminal mutation, then swaps the complete frame into place. A failed build
retains the previous frame and reports the failure; it must not publish half a
frame or lose the dirty work needed for a retry. Retain the current frame and any
frames held by in-flight painting/capture, rather than queueing every update.

Local focus, blink phase, IME preedit, and search UI can cause repaints, but their
terminal coordinates and content must refer to the committed frame. Terminal
mutations remain with the owner. Search highlights require matching buffer and
viewport revisions, rather than overlaying live search coordinates on old cells.

## Cursor contract

Expose Ghostty's resolved visibility, position, visual style, blink flag, color,
and cell footprint through the safe wrapper. Initialize Ghostty's default shape
and blink from user configuration. Let Ghostty resolve application overrides,
save/restore, buffer changes, and reset semantics.

The presentation layer uses those resolved values. User configuration supplies
defaults; it must not continuously override application commands. Cursor
visibility requires both terminal permission and the existing local focus/blink
policy. A blink or focus event cannot resurrect a terminal-hidden cursor.

Implement block, hollow block, bar, and underline drawing with correct cell
geometry, including wide cells. A filled block must preserve readable text under
it through cursor text coloring/redrawing, rather than merely covering a glyph
with an opaque rectangle. Keep cursor placement, glyph selection, and pixel
rounding on the same geometry as the text. IME composition gets an explicit local
caret policy anchored to that same frame.

Only schedule blinking when the committed cursor is eligible to blink. Input
may reset the phase, but cannot change protocol visibility or shape.

## Presentation gate

Read synchronized-output mode from Ghostty. Never scan PTY bytes with a second
ANSI parser, and never use pipe-read boundaries as application-frame boundaries.

| State/event | Required behavior |
| --- | --- |
| Synchronization inactive | Publish changed terminal state at a presentation opportunity. |
| Synchronization active | Continue parsing, input encoding, replies, and effects; retain the committed terminal frame. |
| Synchronization ends | Make the latest state eligible for publication; wake presentation. |
| Repaint while active | Reuse committed content; do not refresh any terminal-backed layer. |
| Suspension deadline expires | Reset the actual Ghostty mode, then publish recovery state. |
| Resize | Apply terminal resize through the owner, invalidate suspension bookkeeping, and publish with matching geometry. |
| EOF/reset | Complete accepted output handling and release any held presentation; cancel obsolete deadlines. |

Use a one-second maximum presentation hold as an explicit recovery policy,
informed by the pinned Ghostty application's one-second sync reset. The protocol
does not prescribe one universal timeout. Associate timeout work with a suspension
generation so an old timer cannot terminate a later update. Do not indefinitely
extend the hold just because output continues.

A begin/end pair entirely inside one parse batch needs no artificial delay. If a
batch ends inside a later synchronized update, retain the prior committed frame;
intervening completed states may be coalesced. Rendering every application update
is not required. Publishing an unfinished synchronized state is forbidden except
for an explicit recovery event such as timeout or resize.

For unsynchronized applications, process the stream normally and honor cursor
visibility. Do not invent frame boundaries using idle delays or app-name rules.

## All presentation paths share the contract

`prepare_terminal` resolves fonts and applies resize before rendering.
`publish_terminal` is the sole live-state builder. `paint_terminal` reads the
owned `Rc<TerminalFrame>` and local UI state; it never updates Ghostty or the
reusable render snapshot. The existing direct canvas painter is preserved.

`input_cursor_bounds`, preedit placement, and cursor painting share
`TerminalFrame::cursor_footprint`, including wide-tail normalization. During
composition, the local preedit replaces the terminal cursor at that anchor;
the terminal cursor is suppressed until composition ends. Candidate placement
does not mutate a render snapshot. Hidden protocol cursors remain hidden even
when input resets the local blink phase.

Live and offscreen painting use the same presentation input. Presented captures
retain the frame actually submitted to the GPU, its output revision, geometry,
and local appearance state. Keep raw live diagnostics available separately and
label them as live observations. A request for a newly presented frame must wait
or time out while synchronization holds; it cannot force a partial commit.

Resize acknowledgements still distinguish requested, terminal-applied, PTY-
acknowledged, and presented geometry. A frame must never pair old rows with new
cursor cell metrics. Preserve the existing ordered PTY I/O and EOF handling.

## Implementation details

- `Snapshot::cursor` exposes protocol appearance through the safe Ghostty module;
  `Terminal::set_default_cursor` initializes configuration defaults.
- The existing pinned Zig bridge calls exported `vt_write_until_ground` and
  `vt_write` functions, counting false-to-true mode transitions at native parser
  ground boundaries. It divides printable runs at ESC only to enter the native
  parser, without interpreting ANSI sequences or UTF-8 itself. This detects
  end/begin pairs inside one PTY chunk and protects newer holds from stale timers.
- Parsing remains bounded by the existing drain-turn budget. Publication happens
  at rendering/offscreen opportunities, coalescing completed updates. Mode polling
  at drain completion starts the timeout even when the pane is not being painted.
- A builder failure keeps the old frame and dirty work, logs the error, and retries
  after 16 ms without requiring further output. This is a failure retry, not an
  output batching delay. Recovery forces a complete native copy because a failed
  update may have consumed row dirty flags. Failed resize retains the previous
  frame's rows and metrics and retries the resize before publication. Frames retain
  owned strings and `Arc<RenderImage>` handles.
- Search ranges carry output revision and grid geometry; stale ranges are excluded
  from a new frame. Selection text and hyperlink targets are committed with cells.
- `PaintedTerminal` retains the frame and local appearance used to construct the
  element tree, separately for live and offscreen painting. Capture metadata
  therefore stays correct even when prepaint resize
  commits a newer frame. Offscreen metadata is built after choosing the painter's
  frame. Capture serialization copies owned cells rather than walking Ghostty again.
- `snapshot --frame next` waits through active holds, rebases its redraw request
  after release, and times out instead of forcing a partial commit. Presented and
  offscreen captures can reuse retained content. Raw reads/state are explicitly live.
- `ctl capabilities` reports host synchronization/cursor support and distinguishes
  direct pixel graphics from unsupported virtual placements. Ghostty's conservative
  default DA1 reply remains unchanged; native mode queries observe actual recovery.

There is no new row cache, terminal worker, channel architecture, or runtime
application-name detection. The frame builder and VT/resize calls still run on
the existing UI owner, so arbitrary workloads can exceed the service budget.

## Acceptance

Use deterministic stream replay and a controllable clock. Check actual rendered
output as well as parser state. Short protocol fixtures should be replayed at
every byte split; larger sanitized Amp traces should use fixed and randomized
chunking. Chunking may change how many complete frames are coalesced, but must
not change final terminal state or leak unfinished synchronized content.

- Hidden cursor movement over labels and animations never produces cursor pixels;
  show/restore resumes at the correct position.
- Shape, steady/blinking requests, user defaults, reset, primary/alternate screen,
  wide cells, focus, scrolling, and IME composition behave consistently.
- Long synchronized updates remain visually frozen across blink, focus, capture,
  search, and unrelated GPUI invalidations while input and replies continue.
- Back-to-back updates, split CSI sequences, timeout with no further output,
  stale timer delivery, resize, reset, and EOF cannot freeze or corrupt a pane.
- Text, graphics, cursor, and capture metadata always describe the same terminal
  revision. Native raster checks cover block-cursor glyph contrast and geometry.
- Windows Amp animation/typing, PowerShell editing, a modal editor, resize, and
  multiple busy panes pass real-GUI checks. Separate direct-VT tests from ConPTY
  tests so transport limitations cannot conceal renderer defects.
- Run formatting, check, Clippy, tests, existing control/capture smoke checks,
  and compare frame/UI-service measurements against the established baseline.

No runtime Amp heuristics, guessed input-box coordinates, blanket cursor hiding,
or extra output sleeps belong in this design.

## References

- [Synchronized-output semantics and timeout discussion](https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md)
- [Ghostty's synchronized-output guidance](https://ghostty.org/docs/help/synchronized-output)
- Pinned source: `ghostty/src/renderer/generic.zig:1197`,
  `ghostty/src/termio/Thread.zig:38`, `ghostty/src/terminal/render.zig:423`.
