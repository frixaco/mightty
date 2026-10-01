# Terminal output scheduling implementation plan

Status: scheduling/completion fixes, direct terminal painting, ReleaseSafe
Ghostty development policy, and the committed terminal presentation boundary
implemented. The earlier sections record the preceding scheduling/performance
work; the presentation follow-up records the current behavior and verification.
Zig Debug remains opt-in.
Updated October 1, 2026.

Fix UI starvation under sustained terminal output with bounded processing and
an explicit scheduling break. Validate that change before adding resize
coalescing or moving terminal ownership to a worker. Keep the existing GPUI
renderer, shell bridges, bounded queues, and Ghostty safety invariants.

## Evidence and decision

`TerminalWidget::start_output_task` batches approximately 256 KiB, then loops
back to `recv_async().await`. A ready receiver does not suspend, so successive
batches can occupy one UI-thread poll indefinitely. A probe using the current
receive and batching code consumed a full 2 MiB queue in one poll; adding a
nonzero async pause returned control after one batch. This confirms the
scheduling mechanism, not an independent reproduction of the reported Amp run.

Ghostty revision `4c6215bb8ee186b5c829457a9a9a9c936f2337bf` parses off the UI
thread and explicitly hands terminal access to a waiting renderer between
batches. Its renderer copies state under a lock and finishes processing its
snapshot after unlocking. Those application mechanisms are not supplied by
our synchronous `libghostty-vt` calls.

Port bounded work and explicit scheduling first. Ghostty's conditional 1 ms
mutex handoff is not an unconditional sleep or a parser time budget. Its POSIX
gathering pipeline and 3 ms gathering budget solve a separate throughput
problem and are outside this change.

Relevant implementation:

- [Output task and lifecycle](../src/widget/mod.rs)
- [PTY reader and control worker](../src/widget/pty.rs)
- [Live painter](../src/widget/render.rs)
- [Pane removal](../src/pane_container.rs) and [exit selection](../src/split.rs)
- [Ghostty handoff](../ghostty/src/renderer/State.zig)
- [Ghostty read loops](../ghostty/src/termio/Exec.zig)
- [Ghostty snapshot preparation](../ghostty/src/renderer/generic.zig)
- [Ghostty integration invariants](ghostty-integration.md)

## Phase 1 Fix scheduling and preserve final output

### Process one bounded turn

Refactor `start_output_task` locally rather than adding a scheduler abstraction.
Remove the intermediate batch vector: apply the first received event, then
receive and apply further events only while the turn has budget remaining.

Initial tuning values are a 256 KiB byte budget, a 2 ms processing budget, and
a 1 ms async pause. These are proposed starting values, not measured optima.

1. Await the first event without holding a strong widget reference.
2. Start the turn clock before applying events. Include VT parsing, callbacks,
   and other synchronous application work in the measured interval.
3. Apply events in order. After each event, stop if either the elapsed-time or
   byte budget is exhausted; otherwise try receiving the next queued event.
4. Notify GPUI once for the turn. Preserve necessary semantic events such as
   title changes and clipboard effects; do not introduce per-chunk repaints.
5. Release the upgraded entity and all terminal borrows before awaiting a
   nonzero timer. Use GPUI's executor timer and clock so the existing test
   executor can control time. Then return to receiving output.

Take the scheduling break after every completed output turn, even if the queue
looked empty at its end: newly arriving output must not bypass it. When no
output arrives, wait on the receiver rather than running a polling timer.
Terminate cleanly when the widget disappears or its update fails.

Do not substitute `std::thread::sleep`, a zero-duration timer, `cx.notify()`, or
a self-waking yield for the scheduling break. Verify the chosen timer against
the real Windows message loop, including several busy panes.

The time budget is cooperative. One current event can contain 32 KiB, and an
individual `vt_write` cannot be interrupted. Measure those calls during GUI
validation. If they exceed the responsiveness target, feed retained output in
smaller slices, starting at 4 KiB, and check the budget between slices. Keep
one pending buffer and offset; consume its suffix before receiving another
event. Preserve UTF-8 and escape parser state, effects, and exact `output_seq`
accounting. Do not add a second queue or discard terminal bytes.

### Complete output before automatic pane removal

This is required for the scheduling change, not an optional cleanup. Today the
reader sets `exit_flag` before the UI has consumed all queued output, and
`has_exited()` exposes that flag to pane removal. Yielding lets the exit task
run while final output is still queued. An exit signal from another pane can
also cause all panes to be inspected.

- Make automatic removal depend on UI-observed output completion. Treat the
  worker's exit flag as a status hint, not proof that queued output is applied.
- Deliver reader completion after its last output event, including its error
  if reading failed. The current one-shot 100 ms completion send may time out;
  make delivery reliable while the consumer is alive, using the existing
  timeout-and-stop pattern so shutdown cannot hang.
- Distinguish a control-worker failure from reader completion. Record the
  failure promptly, allow teardown to finish the reader, and drain accepted
  output before declaring the pane ready for automatic removal.
- Audit `has_exited`, `set_exit_signal`, `mark_exited`, channel disconnection,
  and their callers together. A temporarily empty queue is not EOF. Preserve
  immediate launch-failure reporting and explicit user-close behavior.
- Keep ConPTY's existing shutdown output draining and handle cleanup intact.
  Retain the final pane capture only after its accepted output is applied.

### Add focused regression checks

Use the existing GPUI test support and test terminal construction. Leave a
runnable regression against the actual output task, not a copied batching
algorithm. No new test framework or generic scheduling test harness is needed.

| Check | Required result |
| --- | --- |
| Preloaded output exceeding one turn | The task parks with output still pending; unrelated foreground work can run; advancing the clock resumes processing |
| Ordered output followed by EOF | Every accepted byte is applied exactly once before completion; the final marker and output sequence are retained |
| Multiple panes and early worker exit flag | Another pane's exit cannot remove a pane whose final output is pending |
| Reader or control failure and widget close | Errors remain visible; completion cannot be silently lost; cancellation does not retain the widget or hang workers |

If slicing becomes necessary, add a small case splitting UTF-8 and escape
sequences across slices. Reuse existing terminal tests where possible.

## Phase 2 Validate the real Windows GUI

Extend `tools/control-smoke.ps1` with one opt-in sustained-output case, using
its disposable `--test-instance --data-dir` setup and cleanup. Avoid a second
control framework. Run unchanged and changed builds against the same workload
and record the build profile, hardware, window size, and font settings.

Use a deterministic producer with continuous ordinary output and repeated
alternate-screen redraws containing OSC 8 hyperlinks. Keep output active for
at least 30 seconds and prove that bytes continue to be consumed during UI
checks. Start checking before the first resize. Exercise a busy pane beside
an idle pane, then several busy panes. The producer must be stoppable without
depending on the potentially starved UI.

During output, exercise sidebar or palette input, pointer interaction, divider
and window resizing, tab switching, and a newly prepared presented frame.
Reuse the existing control and capture paths; advancing an output counter or
re-presenting an old frame alone is not evidence of UI responsiveness. Use an
external watchdog so a stuck control request cannot hang the test runner.

After stopping output, verify the final marker, final output sequence, EOF,
and automatic removal in a split pane. Close a pane during active output too.
Repeat with actual Amp in an isolated instance; record this separately from
the synthetic test and do not claim it passed if unavailable.

### Acceptance and measurements

- No indefinite freeze in Debug or Release. Input, resize, and new-frame
  requests complete while output is still active. Start with a 1 second
  watchdog per control operation; this detects starvation, not smoothness.
- For Release, target UI event and frame-progress gaps below 50 ms at the
  95th percentile and no repeated gaps above 250 ms on the test machine.
  These are proposed acceptance targets. Measure in-app timing or frame
  timestamps separately from command startup and PNG readback overhead.
- Record maximum single-parse-call and output-turn time, output throughput,
  and idle behavior. Debug integrity-check cost must be reported separately.
  Use temporary counters or a bounded diagnostic summary, not per-event logs.
- Confirm correctness, bounded memory, complete final output, and cancellation
  before tuning for throughput. Keep idle output processing wake-driven.

If Release meets these targets after the local fix, stop. Do not add a worker
solely to match Ghostty's architecture. If it fails, attribute the time to
parsing, resize, snapshot preparation, painting, or message scheduling before
choosing the next change.

## Conditional follow ups

### Resize coalescing

Add this only if validation shows repeated terminal resizing is a material
remaining cost. Keep only the latest pending interactive size and flush it on
a short timer; Ghostty's 25 ms interval is a comparison point, not a required
value. Update terminal dimensions and the PTY consistently, and ensure the
last size is applied when dragging stops.

Keep control-request semantics intact: a resize acknowledgement must describe
the size actually applied. Explicit control layout requests should flush the
pending interactive resize or use an ordered immediate path. Never return an
old acknowledgement for a newer layout token. Add one rapid-resize check that
verifies final geometry and acknowledgement correctness.

### Synchronized output

More frequent scheduling breaks may expose intermediate TUI redraws. Verify
DEC synchronized-output behavior during Amp and synthetic testing. Ghostty's
application guards snapshot updates and has a recovery timer; our current live
painter does not implement that guard.

If intermediate frames become visible, add a focused change that retains the
last completed terminal frame while continuing to process output and UI
events, then refreshes when synchronization ends. Include bounded recovery if
the application never ends synchronization. Keep this separate from scheduling
fairness and preserve capture metadata matching the frame actually displayed.

### Terminal worker decision

Consider a worker if bounded turns and reasonable slice sizes still fail
responsiveness targets because individual parsing calls occupy the UI, or if
meeting latency targets causes an unacceptable measured throughput loss.
Prefer Release evidence; debug-only integrity costs do not alone establish
the need for an architectural rewrite.

The narrow design is a dedicated terminal owner consuming the existing PTY
output and ordered terminal commands. It creates, uses, and drops Ghostty
objects on its own thread, preserving `!Send`, `!Sync`, private FFI, and lending
iterator guarantees. GPUI consumes owned Rust presentation data with one
latest snapshot per pane and coalesced notifications. Output bytes and command
results are never coalesced away.

Keep the existing reader, control worker, and GPUI renderer unless measurements
justify changing them. Bound worker processing so input, resize, and snapshot
requests cannot starve there either. Before implementing this phase, map
mode-dependent input encoding, selection, search, graphics, clipboard effects,
diagnostics, frame capture, and exit ordering across the ownership boundary.
Do not move raw Ghostty pointers across threads or add `unsafe Send`.

## Delivery and verification

Deliver Phase 1 with its regression checks, then the opt-in GUI stress case and
measured results. Keep conditional changes in separate, evidence-backed steps.
No new dependencies, Ghostty revision change, generated-binding edits, build
mode changes, or GPUI vendor changes are planned for the scheduling fix.

Run the repository checks after implementation:

```powershell
mise exec -- cargo fmt --all -- --check
mise exec -- cargo check
mise exec -- cargo clippy --all-targets -- -D warnings
mise exec -- cargo test
mise exec -- cargo build
pwsh -NoProfile -File tools/control-smoke.ps1
mise exec -- cargo build --release
```

Run the new opt-in stress case against both binaries and record its exact
invocation when implemented. The handoff should state which workloads passed,
observed latency and throughput, whether Amp was exercised, and whether any
conditional follow-up is supported by the measurements.

## Implementation results

The pump applies events directly, stops at 256 KiB or 2 ms, and awaits GPUI's
nonzero 1 ms timer without retaining the widget. The time limit is cooperative:
one event can exceed it. A single notification follows each turn. Completion
and reader errors use an ordered, retrying, cancellation-aware event delivery;
control errors do not declare output complete. Automatic removal reads only
UI-observed completion, including when a reader finishes before widget setup.

Real GUI validation also exposed an existing ConPTY lifecycle gap: the control
worker waited indefinitely for another command after its child exited, keeping
the output pipe open. It now checks process exit during a 250 ms command receive
timeout and closes the pseudoconsole. The reader continues forwarding output
during that close and sends EOF afterward. This introduces four idle control
worker wakeups per second per Windows pane; Unix command reception, output
reads, and the UI output task remain wake-driven. A combined native wait is
deferred unless that cost matters.

Regression checks cover the actual GPUI task parking with pending output,
unrelated foreground work, an early reader exit flag, ordered errors/EOF,
exact byte accounting and final text, widget release, completion backpressure,
cancelled delivery, and ConPTY child exit without another control command.
The full suite passes 145 tests. Formatting, check, Clippy, Debug/Release builds,
and the original Windows control smoke test pass.

New repeatable checks:

```powershell
pwsh -NoProfile -File tools/control-smoke.ps1 -OutputStress
pwsh -NoProfile -File tools/control-smoke.ps1 -Executable target/release/mightty.exe -OutputStress
pwsh -NoProfile -File tools/control-smoke.ps1 -Amp
pwsh -NoProfile -File tools/control-smoke.ps1 -Executable target/release/mightty.exe -Amp
```

The synthetic check runs at least 30 seconds, starts with one busy pane beside
an idle pane, then adds two busy panes. It checks advancing output and newly
prepared frames, palette/sidebar input, window and divider resizing, tab
switching, closing an active producer, and final text/sequence/EOF before
automatic removal. A stop file controls producers independently of the UI.
The Amp check composes and erases characters without submitting a prompt,
requests new frames, resizes, toggles the sidebar, and drags the divider.

The external watchdog allows 1 second for ordinary control operations, 5 seconds
for PNG capture/readback, and 10 seconds for process launch/close. Cold instance
discovery has a 60 second startup deadline. Debug exceeded the original 1 second
PNG watchdog; this is not counted as a one-second frame-latency pass.

Test machine: Ryzen 7 5800X, Radeon RX 9070 XT, Windows x86-64; default
JetBrainsMono Nerd Font Mono at 16 px, DPI scale 1.0, 800×600 startup and
1000×700 / 1100×740 resized windows. Amp version:
`0.0.1790856055-g589fab`. Ghostty revision and build modes are unchanged.

| Measurement | Release synthetic run | Debug synthetic run |
| --- | --- | --- |
| Recorded newly prepared captures | 65 over about 30 seconds | 15 over about 31 seconds |
| Longest output turn | 2.07 ms | 135.15 ms |
| Longest event, including parsing and effects | 1.30 ms | 135.15 ms |
| Final applied output, naturally exited busy panes | about 93.3 MB and 2.64 MB | about 1.67 MB and 0.94 MB |

Counters are printed once per completed output stream to stderr. Event timing
is an upper bound on parsing time, not a separately measured `vt_write` cost.
ConPTY emits screen updates, so producer bytes are not equivalent to PTY bytes.
The exact-once regression uses the event channel; the GUI check verifies final
markers and nondecreasing output counters rather than assuming equal producer
and PTY byte counts.

The Release control timings, including CLI startup, discovery, and transport,
were approximately: keyboard p95 60 ms/max 70 ms; resize p95 55 ms/max 63 ms;
PNG capture p95 115 ms/max 120 ms. These are end-to-end command timings, not
in-app input or frame-progress gaps. The proposed in-app p95 <50 ms target,
idle CPU impact, and before/after GUI throughput comparison have not been
measured. The original ready-receiver mechanism probe remains the baseline
evidence for starvation.

At this stage, no parse slicing, resize coalescing, synchronized-output change, or terminal
worker was added. Release events stayed small; these runs did not establish
a need for those conditional changes. Amp validation covers TUI redraw/input
and resizing, not a live model/tool workload or a full synchronized-output
conformance test.

## Follow-up: Debug Amp is still laggy

The user reproduced unusable lag with `cargo run`. Fresh isolated runs of the
actual Amp home animation and input/resize confirm that the scheduling change
does not fix responsiveness. Temporary instrumentation measured synchronous
`vt_write`, snapshot updates, painter preparation, capture metadata, GPUI's
whole frame, and an independent 10 ms UI heartbeat. Instrumentation was removed
after measurement; the previous implementation was preserved.

Baseline artifacts:
`C:\Users\frixa\AppData\Local\Temp\mightty-control-33fec81f-15e5-43cc-90f3-e5b37e842145`.
Parser-isolation comparison artifacts:
`C:\Users\frixa\AppData\Local\Temp\mightty-control-00b7cd46-ab19-4f51-a50e-b771cae8f9ff`.
The comparison changed only Ghostty's build mode to ReleaseSafe, leaving Rust
and GPUI unoptimized. These are separate runs, not a controlled benchmark of
identical PTY bytes; frame distributions include startup and resize frames.

| Measurement | Debug Rust + Debug Ghostty | Debug Rust + ReleaseSafe Ghostty |
| --- | --- | --- |
| Whole frame median / p95 | 64.09 / 84.72 ms | 64.51 / 83.72 ms |
| GPUI CPU draw median / p95 | 60.91 / 80.88 ms | 61.23 / 79.46 ms |
| Renderer submission/presentation median | 0.60 ms | 0.64 ms |
| Layout/prepaint median | not separately instrumented | 27.39 ms |
| Element painting median | not separately instrumented | 26.26 ms |

Steady baseline Amp parsing was generally below 1.2 ms per call, but parsing
after resize reached 188 ms in the instrumented run. The ReleaseSafe comparison
had no parsing call above 1 ms in its reported intervals, including the resize
stage, yet frame lag remained. The full Debug run exceeded the one-second
control watchdog during resize; the comparison completed its input/resize loop.
Idle heartbeat gaps were approximately 16 ms, while steady Amp regularly held
the UI for 50–80 ms. These gaps are UI service measurements, not physical input
latency or display refresh measurements.

### What differs from Ghostty

1. Our painter creates a positioned Div and StyledText for each style span.
   Amp generated approximately 600 text elements per frame in the comparison.
   Every redraw traverses all cells and rebuilds these elements, even for clean
   rows. The expensive work continues after `paint_terminal` returns, in GPUI
   layout/prepaint and painting. Ghostty retains GPU cell data and skips clean
   rows in `renderer/generic.zig`'s `rebuildCells`. Its shaping cache and direct
   cell drawing avoid a UI layout tree per span.
2. Our 2 ms output budget is checked after a synchronous `vt_write`. It cannot
   bound a call that takes 188 ms. Ghostty's Windows reader parses on an I/O
   thread, using 1024-byte reads and demand-aware handoff between reads
   (`termio/Exec.zig`, `renderer/State.zig`). Its renderer copies state under a
   lock, then performs the remaining frame work on the renderer thread.
3. `build.rs::optimize_mode` selects Zig Debug whenever Cargo DEBUG is true.
   Ghostty's `build/Config.zig` enables `slow_runtime_safety` in Debug only.
   `terminal/Page.zig::setHyperlink` defers `assertIntegrity`, which creates a
   DebugAllocator and verifies the whole page. These checks are intentionally
   expensive. ReleaseSafe retains ordinary Zig runtime safety but disables
   these slow page audits. Ghostty's native build does not universally default
   to ReleaseFast; the build-mode distinction applies there too.
4. Our painter resizes synchronously on the UI thread. Ghostty sends resize
   requests to its I/O thread and coalesces the latest size for 25 ms
   (`termio/Thread.zig`). Our existing PTY command coalescing does not remove
   synchronous Ghostty resize work from the UI thread.
5. Presented-frame capture metadata walks terminal cells again on every
   changed frame. It measured roughly 1–3 ms per pane in steady animation,
   making it secondary here. Any optimization must preserve the exact-frame
   source contract.

### Revised implementation order

First replace terminal style-span Div/StyledText children with direct GPUI
canvas/custom-element painting at the existing fixed cell positions. Preserve
text shaping, fallback fonts, clipping, selection, decorations, graphics z
order, and the existing capture painter. Keep the application shell intact.
If the direct painter still misses the target, retain row drawing/shaping data
using Ghostty's global and row dirty API; invalidate on geometry, fonts, theme,
and presentation state changes. Do not cache a borrow of Ghostty's lending
iterators. Merely caching Strings while rebuilding hundreds of GPUI elements
misses the measured cost. Measure the direct painter before adding row caching.

Separately make normal development builds usable with a ReleaseSafe Ghostty
build, with full Zig Debug reserved for deliberate core-integrity debugging.
This is a build-policy choice, not a substitute for fixing rendering or a
guarantee that arbitrary output cannot block the UI. No build policy was changed
permanently during this investigation.

Then remeasure full frames and UI service gaps during Amp redraw, typing, and
continuous window/divider resizing. If synchronous parsing/resize still causes
unacceptable stalls, move terminal ownership to one worker and publish owned
render snapshots, coalescing resize there. Keep Terminal and its FFI iterators
non-Send/non-Sync; construct and use them on their owning thread. A worker-only
refactor cannot fix the measured 60 ms GPUI frame work and should not precede
the painter fix. Do not port Ghostty's mutex/futex machinery without an actual
shared-terminal design that needs it.

Acceptance must measure UI service gaps and full frame cost, not merely a
successful control response or occasional PNG. Cover sustained Amp animation,
input, continuous resize, multiple busy panes, Unicode/fallback font and
selection fidelity, final-output ordering, and exact-frame capture. The prior
liveness checks remain useful but cannot establish this result.

## Review and performance refactor, October 1, 2026

The complete scheduling/PTY/smoke diff and its callers were reviewed, including
pane removal, resize acknowledgements, live/offscreen rendering, and presented
metadata preparation. The scheduling and EOF changes are retained: bounded turns
need a real asynchronous break, and worker exit is not proof that accepted output
has reached the terminal. The retrying reader completion keeps final bytes ahead
of EOF and remains cancellable during ConPTY shutdown. Windows retains the
250 ms child-exit poll; its four idle wakeups per pane per second are an explicit
tradeoff, not a reason to port a new native wait architecture without evidence.
The existing per-stream timing summary and focused tests earn their small cost.

### Implemented changes

The terminal uses two canvases for its existing spans: background quads and
GPUI-shaped text. Hundreds of positioned Div/StyledText children are removed.
The same text runs, font features/fallbacks, Unicode isolation, cell positions,
selection colors, underline/strikethrough, and per-span clipping remain. Canvas
bounds explicitly reproduce GPUI's device-pixel layout rounding. Search
highlights, graphics layers, cursor, preedit, and overlays retain their order.
Live and offscreen rendering use this same painter. No borrowed Ghostty data is
retained and no Send/Sync or terminal-ownership invariant changes.

Normal Rust development builds now link Ghostty built with Zig ReleaseSafe.
Ordinary runtime safety remains enabled; Ghostty's deliberately slow page
integrity audits are reserved for `cargo run --features ghostty-debug`. Release
and size-optimized modes remain ReleaseFast and ReleaseSmall. Both live state
and capture environment report `build.ghostty_optimize` from the actual build
script invocation. The bridge and VT archive use the same selected native mode.

Fidelity validation found an existing crash when a background capture paints a
search overlay. It also reproduced with the original span painter: a bare element
tree lacked a rendering view for interactive children. The capture boundary now
wraps its owned tree in the existing scratch view type. The snapshot smoke case
keeps search open in its hidden fixture to cover this regression. No GPUI vendor
change remains.

The stress/Amp startup path reads its own published instance descriptor instead
of probing unrelated stale endpoints through global discovery. The ordinary
smoke test still covers `ctl instances`. Empty watchdog responses now include
stderr in the failure. No new test framework or dependency was added.

### Fresh measurements and provenance correction

The first painter comparison inadvertently reused a previously installed native
archive: restoring build.rs with its older timestamp had not forced Cargo to
rerun the native build. Its Rust profile was Debug, but the Ghostty mode could
not be established from the old capture metadata. Those initial numbers must
not be labeled Debug Ghostty. A fresh build-script invocation explicitly selected
Debug, replacing the 11.9 MB linked archive with the 18.9 MB Debug archive; the
subsequent ReleaseSafe build explicitly reported ReleaseSafe. New metadata
prevents confusing the Cargo profile with the native build mode again.

The table below uses the subsequent confirmed builds. An instrumented binary
could select the original span painter or the direct painter. Each ran actual
installed Amp home animation, typed/erased characters without submitting a
prompt, and repeatedly resized the window. Hardware/font/DPI are the same as
above. The three-pane run added two more Amp processes and a 60-move divider
drag through normal GPUI pointer dispatch. All temporary timing code and the
reference painter switch were removed afterward.

| Confirmed run | Full frame median / p95 | UI heartbeat p95 / max |
| --- | --- | --- |
| Debug Rust + Debug Ghostty, original spans | 52.58 / 76.70 ms | 143.23 / 474.51 ms |
| Debug Rust + Debug Ghostty, direct painter | 20.84 / 29.55 ms | 143.21 / 684.02 ms |
| Debug Rust + ReleaseSafe Ghostty, direct painter | 20.68 / 28.44 ms | 40.94 / 65.00 ms |
| Debug Rust + ReleaseSafe Ghostty, three Amp panes and drag | 21.62 / 38.23 ms | 47.13 / 122.96 ms |

Whole-frame timing includes GPUI draw, presented metadata, scene submission /
presentation, and arena disposal. The independently scheduled foreground task
requests a 10 ms timer and measures service gaps. These are separate live runs,
not identical-byte controlled benchmarks. Distributions include idle, startup,
input, and resize stages; they are neither physical keyboard latency nor display
refresh measurements. The original-painter comparison also includes small vector
bookkeeping for the temporary comparison switch. The three-pane run recorded
1,452 changed frames, and no heartbeat gap exceeded 250 ms. Its 63-step pointer
sequence took 4.93 seconds end to end, including CLI/transport and per-step
scheduling; this is not a single-input latency measurement.

Confirmed measurement artifacts:

- Original spans / native Debug:
  `C:\Users\frixa\AppData\Local\Temp\mightty-control-9f3008ba-3ce0-420b-b9e5-b075798229e9`
- Direct painter / native Debug:
  `C:\Users\frixa\AppData\Local\Temp\mightty-control-fa9b4905-49ab-442c-82e3-2bc6570e5394`
- Direct painter / native ReleaseSafe:
  `C:\Users\frixa\AppData\Local\Temp\mightty-control-8214d3fe-71f1-4d50-bc1e-3ce77cefc525`
- Three Amp panes / native ReleaseSafe:
  `C:\Users\frixa\AppData\Local\Temp\mightty-control-95f475b9-b1f1-4362-9b6a-4088efa93a0c`

### Fidelity and remaining limits

A deterministic native raster comparison covers ASCII, bold/italic, fallback
symbols, Nerd Font glyphs, CJK, combining accents, wide emoji, decorations,
blank colored backgrounds, inverse text, wrapping, and selection. After matching
pixel rounding, plain, selected, and offscreen pane captures have zero differing
pixels against the original painter. The search comparison differs only in its
result-label region because one snapshot reports Pending and the other Complete;
terminal pixels and highlights match. A regression checks aligned shared span
edges at 100%, 125%, 150%, and 200% scale. Existing Kitty graphics crop/z-order
and text-input/selection checks exercise the final painter.

The unchanged accepted-output ordering and cancellation regressions remain in
place. The synthetic three-producer smoke run verifies advancing output/new
frames, active close, final markers, sequence counters, EOF, and automatic pane
removal. These progress checks complement the Amp timings; they do not establish
smoothness by themselves.

Row caching, smaller parse slices, resize coalescing, and terminal worker
ownership are deferred. The direct painter still walks cells each changed frame
and, at this stage, capture source preparation performs a bounded second walk. GPUI's
existing shaping cache is used. Default development builds meet the measured
p95 service target here, so another cache or ownership architecture is not yet
justified. A synchronous VT call or resize can still block the UI for arbitrary
workloads, and full Zig Debug intentionally remains slow. Amp validation covers
home animation/input rather than a paid model/tool session; DEC synchronized
output conformance and other machines remain outside these measurements.

### Final verification

Formatting, `git diff --check`, Clippy with all targets and denied warnings,
146 tests (142 library, 3 binary, 1 doc), Debug/Release builds, and
`cargo check --features ghostty-debug` pass. Native raster fidelity and the
snapshot smoke test pass, including hidden search overlays and unchanged live
selection/PTY geometry. The ordinary control smoke test and Debug/Release Amp
smoke cases pass. The Debug output-stress run completes with final output and
EOF; its naturally exited producers report 59.2 MB and 2.62 MB applied output,
with maximum events 1.44 ms and 0.99 ms. These output-summary times include
callbacks/effects rather than isolating parsing.

Startup readiness has a separate ten-second watchdog because an instance
publishes its descriptor before its first pane is fully initialized. The initial
one-second startup failure was corrected without relaxing ordinary one-second
input/resize watchdogs. No output workload had begun at that failure.

Additional direct-VT native raster validation confirms Kitty graphics are hidden
by cell backgrounds at z=-1073741825, cover backgrounds but preserve text at
z=-1, and cover text at z=0 and z=5. The graphics region matches exactly between
live and offscreen captures. The shell-based comparison could not establish
this: ConPTY consumed its APC graphics commands before they reached Ghostty.
The direct-VT fixture instead injected the bytes into the terminal on its owning
UI thread. All temporary injection code was then removed. Its raster artifacts
are at:
`C:\Users\frixa\AppData\Local\Temp\mightty-control-80f8ca97-9c19-413d-8fbe-fe24132c6ee3`.

Cargo also reused an instrumented GPUI archive after its source was restored
with an old timestamp. A fresh GPUI recompilation was forced for both delivered
profiles; temporary profiler/injection markers are checked in the final binaries,
not just in source. That performance-only revision retained no GPUI vendor or
raw-binding changes. The presentation follow-up regenerates a local bridge
declaration and removes the second live-state walk from capture.

## Terminal presentation follow-up, October 1–2, 2026

The accepted [presentation design](terminal-presentation-design.md) is implemented.
One builder publishes owned cells, cursor, selection, search, colors, graphics,
viewport and geometry atomically. Rendering and IME no longer update live render
snapshots. Capture serializes the owned frame actually chosen for painting;
separate live/offscreen paint records prevent offscreen preparation from changing
live metadata. The bounded source-capture budgets remain unchanged.

Mode 2026 gates publication rather than parsing. The existing drain budgets,
one-millisecond dispatch break and ordered EOF protocol remain intact; no new
output sleep or worker was introduced. A generation-safe one-second timer resets
the actual mode on recovery. Resize/reset/EOF release holds. Failed frame building
retains dirty work and retries after 16 ms without new output. Native parsing is
still authoritative; retries force a full snapshot copy if an earlier update
consumed dirty flags, and failed resize retains matching rows/metrics until recovery.
The bridge observes transitions at exported parser ground
boundaries so end/begin pairs in one chunk are handled correctly.

Fresh final measurements use Debug Rust and confirmed ReleaseSafe Ghostty,
installed Amp home animation with character typing/erasing, repeated window
resize, and the same temporary whole-frame/10 ms foreground-timer instrumentation
as the preceding baseline. The three-Amp fixture gives each Amp a third of the
right-hand region and performs a 60-move divider drag. No compiler or concurrent
GUI test was running during these final timing runs.

| Run | Changed frames | Full frame median / p95 | UI heartbeat p95 / max |
| --- | ---: | --- | --- |
| Previous direct painter baseline | — | 20.68 / 28.44 ms | 40.94 / 65.00 ms |
| Owned presentation, one Amp | 487 | 21.78 / 25.51 ms | 35.02 / 161.91 ms |
| Previous three-Amp baseline | 1,452 | 21.62 / 38.23 ms | 47.13 / 122.96 ms |
| Owned presentation, three Amp panes + drag | 1,294 | 18.22 / 25.43 ms | 42.60 / 197.49 ms |

These independent live workloads are comparisons, not identical-byte benchmarks.
The new p95 values meet the 50 ms target here. Maximum service gaps are higher
than the earlier runs but remain below 250 ms. Distributions include startup/cleanup
and do not measure physical keyboard latency. Earlier exploratory presentation runs
varied substantially (one-Amp frame p95 32–38 ms). No percentile improvement is
claimed to be a universal speedup from this architecture. The full-cell copy and
UI-owned VT/resize remain real costs; there is no row cache or worker migration.

Final timing artifacts:

- One Amp: `C:\Users\frixa\AppData\Local\Temp\mightty-control-6a1d3fb4-1aff-44bd-9717-ae8c4e39c595`
- Three Amp panes: `C:\Users\frixa\AppData\Local\Temp\mightty-control-9c0afbd1-5337-43f3-91c6-6799df2dce4d`

The native raster regression and ConPTY smoke fixture are separate. ConPTY's
transport of graphics APC sequences is not treated as proof of renderer support.
Direct pixel placements are supported; virtual placements remain unsupported
and `ctl capabilities` says so. The default native DA1 reply advertises ANSI
color conservatively. Native DECRQM tests verify mode 2026 reports set/reset,
including the host's recovery reset.

Reproduction commands:

```powershell
mise exec -- cargo fmt --all -- --check
mise exec -- cargo check
mise exec -- cargo clippy --all-targets -- -D warnings
mise exec -- cargo test
mise exec -- cargo test native_terminal_presentation -- --ignored --test-threads=1
mise exec -- cargo run --example capture_fidelity
pwsh -NoProfile -File tools/control-smoke.ps1
pwsh -NoProfile -File tools/control-smoke.ps1 -SnapshotOnly
pwsh -NoProfile -File tools/control-smoke.ps1 -PresentationOnly
pwsh -NoProfile -File tools/control-smoke.ps1 -Amp
pwsh -NoProfile -File tools/control-smoke.ps1 -Amp -AmpPanes 3
pwsh -NoProfile -File tools/control-smoke.ps1 -OutputStress
```

The long pointer sequence and deliberate producer waits use explicit watchdogs;
ordinary input/resize operations keep their one-second watchdog. All instances
use disposable storage, with no paid prompt or unrelated process termination.
Ordinary discovery initially hit retained handles of terminated test processes;
creation-time equality alone did not imply liveness. The shared discovery helper
now rejects exited processes before the pipe probe, with a native regression.
Temporary profiling is removed from source and the GPUI vendor patch is restored;
delivered binaries are checked for profiling markers after a fresh rebuild.

Final verification on October 2 passes formatting, check, Clippy with warnings
denied, 156 ordinary tests, the explicitly enabled native Windows GPU regression,
and Debug/Release builds. The native regression compares actual pixels for hidden,
steady/blinking, block/bar/underline/hollow and wide-head/wide-tail cursors, alternate
buffer restore, synchronized repaint suppression, and owned graphics replacement.
Fake-clock tests also cover lost snapshot dirty flags and pending resize geometry.

The profiler-free ordinary control, snapshot, presentation and output-stress smoke
tests pass. Their artifacts are, respectively:

- `C:\Users\frixa\AppData\Local\Temp\mightty-control-c5275e4d-61f5-422d-9b94-0f107769571c`
- `C:\Users\frixa\AppData\Local\Temp\mightty-control-8ab01737-c8ec-4bed-950b-d841c8b4b6c2`
- `C:\Users\frixa\AppData\Local\Temp\mightty-control-5fcced92-d719-4f77-b4c2-1ca6baa35864`
- `C:\Users\frixa\AppData\Local\Temp\mightty-control-69d81f9b-2af4-43e9-aa11-3e8eb550fb9f`

Presentation smoke verifies a real ConPTY producer paused during synchronization,
next-frame capture timeout, recovery with no new output, PowerShell text editing,
and Neovim insert/save through normal UI input. Raster fidelity separately verifies
retained versus freshly presented captures.

After the final recovery changes, Debug snapshot smoke and Release presentation
and three-Amp smoke pass again, without instrumentation:

- Debug snapshot: `C:\Users\frixa\AppData\Local\Temp\mightty-control-1ab406bb-d1cd-493f-a73a-e5521d452229`
- Release presentation/PowerShell/Neovim: `C:\Users\frixa\AppData\Local\Temp\mightty-control-2d21e8c1-7bc5-4dc9-985d-afebcb2041d3`
- Release three-Amp input/resize/drag: `C:\Users\frixa\AppData\Local\Temp\mightty-control-1d4a7898-6a66-4550-8c84-db144db658af`
- Raster fidelity: `C:\Users\frixa\AppData\Local\Temp\mightty-raster-1790882777295`
