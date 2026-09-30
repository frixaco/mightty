# CLI control and diagnostics design

Status: Proposed. These commands and interfaces are not yet implemented.

Add a local control and diagnostics API, exposed through `mightty ctl`, so users
and AI agents can control the running application, inspect persisted state, and
capture the active UI or any tab/pane. The CLI and UI share application
operations; captures use the live renderer.

The API runs inside the existing application. Persisted data is diagnostic
evidence. Session hosting, detach/reattach, and process survival after application
exit remain outside this feature.

## Architecture and shared guarantees

Build on the existing owners and services:

| Component | Responsibility |
| --- | --- |
| [`PaneContainer`](../src/pane_container.rs) and [`Split`](../src/split.rs) | Own windows' tab/pane topology and expose concrete targeted operations |
| [`AppAction`](../src/action.rs) | Adapt existing shortcuts, menus, and palette intents to those operations |
| `TerminalWidget` and safe Ghostty wrappers | Own terminal interaction and prepare immutable presentation data |
| [Activation transport](../src/application/windows/instance.rs) | Supply the Windows user/session access restrictions for a separate control endpoint |
| Application diagnostics | Project owned state, coalesce persistence, sample processes, and coordinate bounded captures |
| [Feedback capture](../src/feedback.rs) | Use the same capture service for the CLI and feedback shortcut |

Existing actions mostly target the active pane, the activation pipe has no
application-result reply, and Windows PNG feedback copies a screen rectangle
that can include an overlapping application. The design replaces these
limitations without adding a second application model.

These guarantees apply across the API:

- **Explicit identity:** runtime IDs belong to an instance. Stale or conflicting
  targets fail; they never fall back to another pane or instance.
- **Intentional side effects:** background control preserves selection unless
  explicitly requested. Observation does not change focus, viewport, selection,
  or terminal/PTY size. Only a direct `window focus` command requests OS
  activation; injected UI events retain their normal handler side effects.
- **Honest observations:** report source, time/version, availability, and
  truncation. Current state, process samples, terminal data, and presented pixels
  are different observations and must not be relabeled as one another.
- **Bounded work:** advertise limits for requests, queues, input bytes, waiters,
  reads, retained outcomes, persistence, and GPU resources. Reject excess work
  before commitment; perform blocking I/O off the GPUI thread.
- **Explicit completion:** replies describe the operation's defined completion
  point and known effects. Partial changes are reported; timeouts do not imply
  rollback and mutations are never automatically retried.
- **One implementation:** typed control operations and UI actions share methods;
  captures share the painter; persisted state is a projection of live owners.

Keep protocol types, target resolution, and diagnostics at the application layer.
Platform transport owns pipe handles/access restrictions; shell modules expose
process observations safely. Build owned snapshots on the GPUI thread, then
serialize/write in background workers. Never serialize GPUI entities, handles,
or Ghostty pointers; preserve the wrappers' non-`Send`/`Sync` and lending
iterator invariants.

Windows is the first transport and pixel-capture implementation. Keep protocol
and serializable models platform-neutral for later Unix support.

## CLI control

Human-readable output is the default; `--json` provides a machine-readable
result. These are independent examples:

```powershell
mightty ctl instances --json
mightty ctl capabilities --json
mightty ctl profiles --json
mightty ctl state
mightty ctl state --json

mightty ctl tab new --window w1 --profile powershell --cwd C:\work
mightty ctl tab select --tab t2
mightty ctl tab move --tab t2 --before t1
mightty ctl tab close --tab t2
mightty ctl pane split --tab t2 --pane p7 --direction left --ratio 0.35
mightty ctl pane resize --pane p7 --edge right --delta-px 80
mightty ctl pane focus --pane p7
mightty ctl pane focus --pane p7 --direction left
mightty ctl pane zoom --pane p7 --enabled true
mightty ctl pane close --pane p7
mightty ctl window focus --window w1
mightty ctl window resize --window w1 --width 1000 --height 700
mightty ctl ui sidebar --window w1 --visible false
mightty ctl ui palette --window w1 --open true --query split
mightty ctl ui search --pane p7 --open true --query error
mightty ctl pane scroll --pane p7 --rows -20

mightty ctl pane send-text --pane p7 --file input.txt
mightty ctl pane send-key --pane p7 --key Enter
mightty ctl pane read --pane p7 --tail 100 --json
```

`capabilities` advertises operations, arguments, result/error schemas, versions,
platform support, and limits. Derive CLI help and capabilities from shared typed
definitions where practical; no plugin or general command framework is needed.
The [protocol reference](#protocol-reference) defines encoding and wire examples.

### Target resolution and identity

The CLI chooses an instance; resolve object IDs and aliases once on its GPUI
thread at dispatch:

| Input | Resolution |
| --- | --- |
| Explicit instance | Connect only to it and validate its identity |
| Inherited instance | Use when none is explicit; a stale value fails |
| Neither | Use the sole reachable instance; otherwise report unavailable or ambiguity with candidates |
| Explicit pane/tab | Derive current parents and validate any explicit ancestors |
| Only an explicit ancestor | Use its selected descendant when needed |
| Object target omitted | Use a matching inherited pane and derive its parents; otherwise require a unique eligible target |
| Explicit `active` | Use the instance's last-focused window and its selected tab/pane; fail if unavailable |

An explicit ancestor suppresses inherited descendant IDs. Selecting a different
instance suppresses all inherited object IDs. `state` without object selectors
returns the whole selected instance, ignoring inherited object IDs; explicit
selectors return a subtree. Application selection and OS foreground focus are
separate fields, so `active` does not imply OS focus.

Allocate runtime IDs before shell launch and never reuse them within an instance.
[Workspace restoration](../src/workspace.rs) remaps saved tab/pane IDs and all
references to fresh runtime IDs. Resolve through existing owners; add an index
only if lookup cost warrants it.

New shells receive `MIGHTTY_INSTANCE_ID`, `MIGHTTY_WINDOW_ID`,
`MIGHTTY_TAB_ID`, and `MIGHTTY_PANE_ID`. These identify launch context; derive
current ancestry from the live pane if it moves. Existing handed-off processes
cannot receive these variables retroactively; discover their IDs through state.

### Operation semantics

- Split left/right/up/down relative to the target pane; `--ratio` is the new
  pane's share. Creation preserves selection unless `--focus` selects the new
  target within its window.
- `tab select` and `pane focus` change application selection; directional focus
  starts from the named pane. `window focus` reports actual OS activation.
- Closing an unselected target preserves selection. Closing a selected pane
  chooses the next tree leaf, then the previous if necessary. Its last pane
  closes the tab; a selected tab's replacement is the previous tab, then next.
  The final tab closes its window. Return resulting IDs and removals.
- Resize takes an edge and signed logical-pixel displacement; positive grows
  outward. Move the nearest applicable split divider, reporting actual bounds
  and minimum-size clamping. An unavailable edge fails.
- Window resize sets client-area logical dimensions and returns actual bounds,
  DPI, and OS clamping. Zoom/sidebar/overlay commands use explicit desired state.
- Opening palette/search changes input focus within its window; closing restores
  the previous valid target. A hidden pane's search can be prepared without focus
  or revealing its tab.
- Scroll uses physical rows: negative toward history, positive toward the bottom.
  Also support explicit top/bottom destinations; return viewport and clamping.

### Terminal input and UI interaction

| Surface | Routing |
| --- | --- |
| `pane send-text`, `pane send-key`, `pane input` | Named terminal through Ghostty encoding and paste policies |
| Typed control operations | Shared application methods |
| `ui key`, `ui text`, `ui pointer` | Normal GPUI focus, shortcut, text-input, and hit-testing paths in the named window |

`pane send-key --key p --mod ctrl --mod shift` sends a terminal key regardless
of overlays. The equivalent `ui key` can open the palette through the configured
shortcut. `ui text` commits text to the focused component. UI replies identify
the receiving focus target and dispatch outcome, without claiming an
unacknowledged PTY write. Terminal text never implicitly presses Enter; unmet
paste-policy requirements fail instead of opening an unattended confirmation.

Pointer events support move, button press/release, and wheel input with explicit
buttons/modifiers. Coordinates are logical pixels relative to the window client
origin. A window layout token guards against stale hit regions. `ui input`
takes a window target and a JSON step file (`--file PATH` or stdin with
`--file -`) for real divider drags and selections; validate balanced transitions and release only the sequence's synthetic held state on
failure. Injection does not move the system pointer and does not test native
keyboard layouts, IME candidate windows, or OS activation. Use isolated instances
for interaction tests.

`pane input --pane p7 --file steps.json` accepts ordered text/key steps;
`--file -` reads stdin. Convenience send commands map to one step:

```json
[
  {"type": "text", "text": "Write-Output ('run-' + 'token')"},
  {"type": "key", "key": "enter", "modifiers": [], "event": "tap"}
]
```

Actual tests use a run-unique marker. Validate the whole sequence, reserve its
byte budget, encode against one terminal-mode observation, and enqueue one item.
Other producers cannot interleave writes between steps, but may write before or
after it; this does not reserve the shell's editing buffer. Sequences contain no
delays, waits, or application mutations. Failure reports completed steps and
known encoded-byte progress within the failing step.

### Layout and preconditions

Calculate geometry independently of painting. Initialize every tab, including
never-displayed tabs, from known window content bounds and resolved font metrics.
Apply split layout and terminal/PTY sizing on creation, split, resize, zoom, window
size, and font/DPI changes. Expose computed bounds, Ghostty grid, acknowledged
PTY size, and last-presented bounds separately.

Layout mutations accept `--if-layout TOKEN`. Tab tokens cover topology, grid,
and geometry; window tokens cover layout, DPI, and relevant UI hit regions.
Tokens include instance/owner identity and advance only on relevant changes,
not unrelated PTY output. Compare immediately before commitment on the GPUI
thread; a mismatch returns `precondition_failed`, no effect, and the current
token. Returned tokens describe observations, not locks.

### Reading terminal content

```powershell
mightty ctl pane read --pane p7 --viewport --format text
mightty ctl pane read --pane p7 --buffer primary --tail 100 --json
mightty ctl pane read --pane p7 --viewport --format cells --json
```

Reads allocate no render target or capture files. `--viewport` returns the
active buffer's visible rows; `--tail N` returns the selected buffer's last N
retained physical rows. Default to the active buffer; unsupported explicit
primary/alternate selection fails.

Return one consistent observation with identities, row range, total retained
rows, output cursor, and truncation details. Bound rows/bytes and report actual
range, omitted rows, and reason. Initial support needs viewport/tail reads,
without a stateful scrollback paging service.

Text preserves spaces, emits wide/combined text once, and uses LF between physical
rows. JSON includes soft-wrap flags; cell output retains columns, widths,
continuations, styles, and blank backgrounds. Text format without `--json`
writes only text; JSON wraps content/metadata in the normal envelope, which cell
output always uses.

### Launches and isolated tests

Creation and splitting use one `LaunchSpec` contract: resolve the selected/default
profile, then apply `--cwd`, environment overrides, and optional
`--exec PROGRAM -- ARG...`. Explicit execution replaces the executable and
argument list, preserves separate argv values, and receives no injected
shell-integration arguments. Overrides never rewrite profiles.

Support repeated `--env NAME=VALUE` and `--unset-env NAME`. Inheritance is the
default; `--env-mode empty` includes only supplied values and documented mightty
identity variables. Fixtures supply other required system variables. Preserve
launch-directory validation; resolve CLI paths relative to the CLI's directory.

The runner starts a real GUI instance explicitly:

```powershell
mightty --test-instance --data-dir C:\temp\mightty-case
```

Require the data directory and use its settings, workspaces, and diagnostics,
ignoring personal configuration overrides. Bypass normal single-instance
activation, expose a separate control endpoint, and disable global hotkeys and
default-terminal COM registration.

The runner discovers its launched PID through `instances`. Startup readiness
means fonts, initial geometry, and dispatch are initialized, not shell-prompt
readiness. Fixtures use explicit argv (for example PowerShell `-NoProfile`),
their own ready/completion markers, and fixed font/theme/blink settings. DPI
tests use and report actual display scaling.

The runner cleans up only its own files/processes and preserves failed bundles.
The application never automatically deletes the data directory.

### Dispatch and completion

Each GUI process, including `-Embedding`, publishes a control endpoint independent
of `PrimaryInstance`. Keep the existing activation endpoint for startup. Publish
a discovery descriptor with instance ID, PID, process creation time, endpoint,
and protocol version; validate it through a bounded handshake and ignore stale
entries during discovery. The CLI never starts the GUI implicitly.

Transport workers validate bounded frames and enqueue typed requests. GPUI
resolves targets and mutates owners; asynchronous workers acknowledge I/O.
Use bounded queues, connection deadlines, and cancellation.

Preserve serialized `AppAction` compatibility as intent adapters, rather than
using it as the wire protocol. Resolve toggles once; translate existing resize
units using their current 8 horizontal/18 vertical logical pixels. UI invocations
resolve current focus, while the CLI passes its target. Never temporarily select
a background tab to reuse an active-pane action.

| Operation | Successful reply means |
| --- | --- |
| Create tab/split | Shell launched, topology committed, initial terminal/PTY dimensions established |
| Select/reorder | Requested selection/order committed |
| Resize/zoom | Layout and Ghostty sizes changed; affected PTY resizes acknowledged |
| UI event | Normal dispatch finished; resulting render/launch/shell work has separate readiness |
| Read | One bounded terminal observation copied |
| Input | Encoded bytes written to PTY; execution/consumption is not implied |
| Close | Removal committed and cleanup requested; exit observed separately |
| Snapshot | Requested frame and bundle publication completed |

Carry typed acknowledgements through the PTY worker. Bound queued and in-flight
input bytes, reserving capacity before dependent mutations. Later PTY failure
reports partial topology/geometry effects and requested versus acknowledged
sizes. A failed shell launch cannot appear as a successfully created empty pane.

Input replies report encoded-byte progress and an opaque `output_cursor` for the
instance/pane/output sequence immediately before enqueueing. State reads also
expose cursors. Replies may arrive after unrelated changes; their observation
tokens identify the operation's result.

Cancellation before dispatch can return no effect. After commitment, disconnect
does not undo work. Timeout/disconnect after transmission without a definitive
reply returns `outcome_unknown`. Request IDs correlate diagnostics; there is no
deduplication cache. When closing the application, allow a bounded flush of its
reply and final state before stopping the endpoint.

### Waits and events

```powershell
mightty ctl wait --pane p7 --text "unique-run-token" --after-output "$cursor" --timeout 30s
mightty ctl wait --pane p7 --condition process-exited --timeout 30s
mightty ctl wait --tab t2 --condition title-equals --value "Build output" --timeout 5s
mightty ctl wait --tab t2 --condition layout-ready --layout "$layout" --timeout 5s
mightty ctl wait --window w1 --condition settings-generation --at-least 4 --timeout 5s
mightty ctl wait --window w1 --condition overlay-open --overlay palette --timeout 5s
mightty ctl events --json
```

Register state waits and perform the initial check together on GPUI. Text waits
register before their incremental scan. Neither recovers transient conditions
that ended before observation. Advertise these fixed predicates and availability:

| Condition | Evidence |
| --- | --- |
| `title-equals` | Chosen normalized tab title equals the supplied string |
| `layout-ready` | Exact token remains current and all affected Ghostty/PTY sizes are applied/acknowledged |
| `settings-generation` | Window settings store accepted at least that generation; existing panes retain their live-reload policy |
| `overlay-open` | Named overlay mounted; a next-frame snapshot checks presentation |
| `prompt-ready` | Supported integration reports the current prompt/input phase, not an old scrollback marker |
| `process-exited` | Pane root process observed exited, independently of output EOF |

Return the satisfying observation; a superseded layout token fails. A TUI may
still be redrawing after PTY resize acknowledgement, so fixtures also wait for
their output/state marker. There is no generic terminal-idle guarantee.

Text waits match a literal in the current active buffer, including retained
scrollback. `--after-output` requires at least one applied byte beyond its cursor;
it does **not** establish that the matched text is new. Validate cursor identity
and reject future sequences. Without a cursor, existing text can satisfy the wait.

Deterministic fixtures emit a unique marker only at completion, constructing it
from fragments so shell echo cannot contain the full marker. Semantic command
completion would require integration capability and command identity.

Use independent incremental Ghostty search handles, not the interactive
`Terminal.search` field. Bound waiter count and per-turn work. An unsatisfied
text wait fails on buffer reset/switch (`buffer_changed`) or removal
(`pane_closed`, with retained outcome). Evicted or transient text between scans
may be missed. Process-exit waits follow the root through retained outcomes;
unavailable observation or expired outcomes fail. Timeouts include last observed
output sequence and lifecycle.

Keep observation identifiers distinct:

| Field | Meaning |
| --- | --- |
| `revision` | Published GPUI state change, including output batches/process observations |
| `output_seq` | Cumulative PTY bytes applied to that runtime pane's Ghostty terminal; never resets |
| `frame_id` | Prepared presentation data, including cursor phase and overlays |
| `layout_token` | Relevant window/tab topology and geometry generation |

`events` atomically subscribes and obtains initial state at revision R, then
emits typed changes with revisions greater than R. Bound subscriber queues;
overflow terminates with `resync_required` and the last delivered revision.
Reconnect for fresh initial state. No historical replay or separate event
sequence counter is required.

## Diagnostic state tree

Expose one serializable projection:

```text
Application instance
|-- build/runtime identity, PID, uptime, settings generations, diagnostics
|-- revision, persistence status, bounded recent pane outcomes
`-- Windows
    |-- bounds, DPI, visibility, focus, active tab, overlays
    `-- Tabs
        |-- title provenance, order, selected pane, zoomed pane
        |-- split tree: axes, ratios, pane references
        `-- Panes
            |-- profile, effective configuration, launch information
            |-- shell-reported title and working directory
            |-- computed/presented bounds, terminal/PTY dimensions, viewport
            |-- cursor, selection, modes, shell-integration status
            |-- PTY lifecycle, output sequence, exit status
            `-- root process and observed descendants
```

### Provenance

Keep these title stages distinct:

1. Shell-reported title from Ghostty, with observation time and known source
   limit; this does not preserve unlimited raw OSC bytes.
2. Normalized title and applied transformations: currently remove controls, limit
   to 128 Unicode scalar values, and trim. Record which rules changed the value.
3. Chosen tab title, source pane/profile, and fallback reason.
4. Frame label with index/attention decoration and layout/clipping diagnostics.

Record each pane's effective configuration, launch settings generation, and
subsequent live-applied fields. Current settings need not describe older panes.
Include resolved profile/executable/argv; directory fields identify configured,
shell-reported local, or remote sources and retain existing launch-path trust
rules. Environment inspection exposes names/availability by default; fixtures
provide their known values for reproduction.

### Lifecycle and processes

Distinguish process exit, output EOF, I/O failure, launch failure, and user close.
The current PTY bridge collapses EOF/failure into `PtyEvent::Exited`; EOF alone
cannot supply an exit code, nor does a finished shell command imply root exit.

Before auto-removal, notify waiters and publish a retained outcome keyed by pane:
removal reason, nullable exit code, process identity, final output sequence, and
bounded text tail. Update it if exit is observed during cleanup. Advertise count,
age, and byte limits; persist outcomes. Targeted state can return one until
expiry, while mutation/snapshot returns `pane_closed`. There is no removed-pane
GPU archive; a retained window frame may still show it historically.

Capture root process identity on launch/handoff. One background sampler per
instance enumerates descendants once and distributes observations: PID, parent,
executable, creation time, lifecycle, and sample time. Pair PID with creation
time where available; merge on GPUI. Windows
[Tool Help](https://learn.microsoft.com/en-us/windows/desktop/ToolHelp/taking-a-snapshot-and-viewing-processes)
provides enumeration and parent IDs.

Expose freshness and coverage. Sampling misses some short-lived processes;
Windows cannot establish full SSH/WSL trees, and access failure is not an empty
tree. Shell builtins require integration metadata rather than child processes.

### Persistence

```text
diagnostics/
  <instance-id>/
    state.json
    events.ndjson
    captures/
```

One writer atomically replaces `state.json` with the latest owned metadata
snapshot. Coalesce before traversing the tree; debounce bursts but publish dirty
state at least once per second during continuous activity when storage is
healthy. Include schema, identity, timestamps, and revision.

Rotate a bounded `events.ndjson` journal of structural/title/directory/process
changes, errors, and control outcomes. Input records contain operation metadata,
not text payloads. The journal is evidence, not a state-reconstruction dependency.

Publish startup state with shutdown unrecorded, then a final dirty-state flush
with an orderly-shutdown marker and bounded deadline. An absent marker does not
prove a crash; preserve the last successful snapshot after abrupt exit.
Report `persisted_revision` and write errors without marking persistence status
alone dirty and creating a write loop. Failures do not block input/rendering.

Bound retained instance metadata independently of requested captures, which stay
until explicitly removed. Keep cells/images in captures; do not continuously
rewrite scrollback or export the full environment.

`state --json` is live. `state --saved --instance ID` reads local persisted data
without contacting the application. Without a selector, saved mode ignores
inherited live context and chooses the latest recorded startup, reporting source,
instance, and timestamps.

## Snapshotting

```powershell
mightty ctl snapshot --window active --frame presented --out .\captures
mightty ctl snapshot --window w1 --frame next --layout "$layout" --out .\captures
mightty ctl snapshot --tab t2 --out .\captures
mightty ctl snapshot --pane p7 --out .\captures
mightty ctl snapshot --pane p7 --frame presented --out .\captures
```

| Target | Contents | Default mode |
| --- | --- | --- |
| Window | Full native-pixel client area: sidebar, titles, splits, selections, palette, app overlays | `presented` |
| Tab | Content layout with panes and dividers | `offscreen` |
| Pane | Terminal viewport with styles, cursor, selection, search, graphics | `offscreen` |

| Mode | Contract |
| --- | --- |
| `presented` | Acquire the latest retained successfully presented frame without requesting redraw |
| `next` | Request a redraw and wait for rendering, presentation, and readback |
| `offscreen` | Render current immutable tab/pane presentation data through the live painter |

For tab/pane targets, `presented` and `next` crop the window frame at the target
bounds, preserving clipping and overlays. Missing targets return
`target_not_in_frame`. Whole-window offscreen capture is outside initial scope.
Explicit modes never substitute another method.

Use window capture for tab-title wrapping/clipping, including inactive tabs'
visible labels. Content-only tab images cannot demonstrate sidebar layout.
Application frames exclude other applications' occlusion. OS-owned popups such
as native IME candidate windows would need a separately identified screen-capture
path.

### Frame acquisition

Latch `presented` before scheduling another paint, preserving stale-glyph,
clipping, or invalidation defects a redraw might repair. Report presentation
time, age, and current window visibility. Hidden/minimized windows may return
a historical retained frame; absent data returns `frame_unavailable`.
For `next`, hidden/minimized windows return `not_presentable`; presentation
timeouts are explicit.

Retain only the latest surface and matching immutable presentation data per
window, under an advertised memory budget. Reuse shared render data and GPU
resources instead of reading pixels to CPU or copying the full application tree
on every paint. Budget eviction makes the frame unavailable. There is no frame
history recorder.

Frame data retains focus appearance, cursor phase, overlays, and effective
title/configuration/font references. Later changes cannot mutate those references.
Process observations belong to separately timestamped live state.

`--layout TOKEN` on `presented` checks the acquired frame's token. For `next`
or `offscreen`, wait for exact layout readiness and validate again when preparing
the frame; supersession returns `precondition_failed`.

Offscreen capture uses geometry and terminal dimensions established by the
independent layout path, including never-displayed tabs. Pending sizing waits
within the deadline or returns `layout_unavailable`/the sizing error. It never
repairs geometry by resizing a PTY. Use current terminal contents and logical
selection/overlay state without simulating OS focus.

The [current renderer](../src/widget/render.rs) also updates geometry and resizes
the terminal. Separate those lifecycle effects from preparation/painting, and
extract owned presentation data through safe Ghostty interfaces, retaining fonts,
graphics, colors, and overlays.

GPU support is the principal technical uncertainty. Prototype retained-frame
readback, next-frame capture, and hidden rendering before fixing the extraction
boundary. Measure retention/copy overhead and establish any required GPUI changes;
do not replace retained-frame capture with redraw if support is difficult.

### Bundle and consistency

Each request creates a uniquely named bundle:

| Artifact | Contents |
| --- | --- |
| `manifest.json` | Versions, mode, target IDs, frame ID/time, dimensions, DPI, frame/live revisions, output cursors, artifact availability/status |
| `image.png` | Pixels of the selected frame or crop |
| `frame.json` | Matching cells, presentation metadata, labels/bounds, styles, layout tokens, available shaping diagnostics |
| `state.json` | Current application metadata with its own revision/observation time |
| `environment.json` | Build/runtime fingerprints and effective configuration |
| `diagnostics.ndjson` | Bounded recent diagnostics relevant to the target |
| `panes/<pane-id>.txt` | Convenience text for each pane in the frame |

Pixels and frame metadata describe the same presentation. Current state may be
newer; unavailable historical fields remain explicitly unavailable rather than
being filled with current values. Cells preserve widths, continuations, blanks
and their styles, and wrapping. The [existing serializer](../src/widget/capture.rs)
skips empty cells and needs extending. Plain text is a convenience export.

The CLI resolves `--out` to an absolute path. Write a temporary bundle, then
publish a finalized directory with a manifest listing each artifact's completion
or error. Partial bundles return available paths, `partial` status, and nonzero
exit status. Never overwrite a bundle.

One capture coordinator bounds queue depth, dimensions, readback bytes, and
retained resources. Return `busy` before accepting work beyond those limits;
release resources on failure/timeout. Route `Ctrl+Shift+F12` through this service
in `presented` mode.

### Visual fidelity

The PNG must preserve actual defects: if a red flower renders as a black box,
capture that black box. Use lossless pixels from the renderer, with color format,
space, and export conversion recorded. Never reconstruct the image from text,
substitute fonts, fix colors, expand labels, or disable clipping.

This includes Nerd Font icons, ligatures, fallback glyphs, emoji, wide/combining
characters, blank backgrounds, decorations, and clipped runs. Off-viewport
characters remain terminal data or require another requested view.

Frame diagnostics distinguish three layers:

| Layer | Data |
| --- | --- |
| Terminal source | Exact Unicode/code points, cell coordinates, widths, attributes, foreground/background, inverse |
| Painter inputs | Font family/size/features/fallback chain, cell metrics, DPI, effective colors, run bounds and clips |
| Raster output | Actual captured pixels |

Include resolved font faces and glyph/cluster data when the shaping backend
exposes them. A configured fallback list does not prove which face was used.
Reuse retained shaping results; collect additional detail during `next` or
`offscreen` preparation. Reshaping current text cannot reconstruct past glyph
selection for `presented`. Mark unavailable optional diagnostics in the manifest
without preventing pixel capture.

For tab titles, retain title provenance, actual decorated label, tab ID, allocated
bounds, ancestor clips, font/line height, and wrap/overflow policy. Include
measured extents and line count when available. These fields explain constraints;
the PNG establishes actual wrapping, ellipsis, overlap, or clipping.

### Reproduction context

Embed mightty version/revision, build mode/target, Ghostty revision/header
fingerprint, and GPUI version at build time; do not run Git in the user's shell
directory. Record OS, backend, available GPU/driver identity, display scaling,
and capture color conversion.

Include captured panes' frame-effective settings, separately identified current
settings generations, resolved launch inputs, and renderer font resources.
Cache obtainable font version/content fingerprints instead of hashing during
painting. Fixture bundles also reference their launch recipe and configuration.

Promote startup, PTY, settings, layout, font, and rendering errors into one bounded
structured diagnostic model shared by state, events, and bundles. Records include
time, subsystem, severity/code, underlying platform errors, and known
instance/window/tab/pane/request/frame IDs. Export a recent tail with range and
truncation status; do not invent frame associations for uncorrelated errors.

## Delivery and validation

Run the rendering feasibility investigation early, independently of control
delivery: prove retained, next, and offscreen capture and measure resource costs.
Record the GPUI boundary and unresolved backend limits before committing to
capture implementation.

Deliver usable increments:

1. **Inspect:** instance identity/discovery, protocol envelope/capabilities,
   minimal live state, and bounded content reads.
2. **Control:** targeted methods, independent layout, launch overrides, typed PTY
   acknowledgements, ordered input, and layout guards. Add isolated GUI launch
   and fixture-based CLI tests with this increment.
3. **Observe:** readiness waits, lifecycle outcomes, process sampling,
   title/configuration provenance, structured diagnostics, persistence, and events.
4. **Exercise UI:** scenario controls and event injection through real handlers.
5. **Capture:** bundles and reproduction context through the proven renderer,
   shared with the feedback shortcut.

Add integration tests with each increment. Historical replay, request
deduplication, general expression predicates, and generic fresh-text provenance
remain outside the initial API.

Two acceptance scenarios connect the interfaces:

- Start an isolated instance, discover state/capabilities, and create a fixture
  in a never-displayed background tab. Submit ordered input constructing a unique
  completion marker, wait after its input baseline, read output/processes/path,
  resize with a guard, and capture the exact layout offscreen without changing
  the selected tab. Retain the recipe/settings/diagnostics.
- Resize a window, exercise sidebar/palette/search, and inject keyboard/pointer
  sequences through real handlers. Wait for state/layout and capture the next
  frame. A defect fixture must prove that `presented` preserves a defect even
  when the next redraw repairs it.

Use this matrix for focused failure and fidelity coverage:

| Area | Cases |
| --- | --- |
| Identity/control | Reorder/restore/restart; stale/conflicting targets; normal/quick-terminal/embedding instances; four directions; nested resize/clamping; background selection/focus |
| Launch/isolation | Exact argv/environment/directory; startup readiness; failed launch; isolated storage; no interference with primary instance |
| Input/events | Terminal versus UI routing; modifiers/taps; real drags/selection; paste policy; concurrent producers; partial/stalled writes; byte limits; cancellation/unknown outcomes |
| Reads/waits | Primary/alternate buffers; wraps/spaces/wide cells; truncation; old/echoed markers; chunked output; concurrent search; buffer switch/removal/timeout; superseded layouts; resize versus TUI redraw |
| State/processes | EOF versus exit; PID reuse; unavailable/remote coverage; retained outcomes/expiry; final text; title normalization/fallback; older effective settings |
| Persistence/streams | Atomic publication/subscription; continuous-output deadline; write failure; subscriber overflow/resync; abrupt exit; offline selection; pruning preserves captures |
| Raster fidelity | Actual glyph/color defects, ligatures/icons, wide/combining text, cursor/selection/search/graphics; title wrap/ellipsis/clipping including inactive labels; multiple DPI settings |
| Capture lifecycle | Never-presented tabs; exact visible crops/overlays; occluded/minimized windows; missing/evicted frames; readback failure; limits/deadlines; unchanged viewport/PTY/focus; frame versus live revisions |
| Reproduction/protocol | Build/font/runtime fingerprints; bounded correlated logs; client-relative paths; schema fixtures/version rejection; UTF-8/BOM/stdin; preserved CR/LF/Unicode; clean JSON stdout |

Run repository formatting/checks and focused Windows runtime tests when
implementing. Keep commands labeled proposed until implemented and verified.

## Protocol reference

### Envelopes and compatibility

The CLI grammar and transport share typed argument definitions, with fixtures
checked against published schemas. CLI-side file/path and environment-context
resolution prepares requests; live object/alias resolution remains on GPUI.
Transport framing must bound each request before deserialization.

Replies include protocol version, request ID, instance ID, state revision,
`ok`, and either `result` or `error`. Errors have stable codes and effects:
`none`, `committed`, `partial`, or `unknown`, with affected IDs and known
progress. Unary JSON commands emit one envelope to stdout; diagnostics go to
stderr and failures exit nonzero.

A resize request:

```json
{
  "protocol_version": 1,
  "request_id": "r17",
  "instance_id": "91cbcc1a-2a1f-4f87-930b-3afc6aec2901",
  "op": "pane.resize",
  "target": {"pane_id": "p7"},
  "args": {"edge": "right", "delta_px": 80},
  "preconditions": {"layout_token": "layout:tab:t2:7"},
  "timeout_ms": 5000
}
```

Tokens are illustrative opaque values. Success includes affected geometry and
the committed layout token:

```json
{
  "protocol_version": 1,
  "request_id": "r17",
  "instance_id": "91cbcc1a-2a1f-4f87-930b-3afc6aec2901",
  "revision": "42",
  "ok": true,
  "result": {
    "layout_token": "layout:tab:t2:8",
    "affected_panes": [
      {
        "pane_id": "p7",
        "bounds": {"x": 0, "y": 0, "width": 800, "height": 600},
        "terminal_size": {"cols": 100, "rows": 30},
        "pty_size": {"cols": 100, "rows": 30}
      },
      {
        "pane_id": "p8",
        "bounds": {"x": 804, "y": 0, "width": 320, "height": 600},
        "terminal_size": {"cols": 40, "rows": 30},
        "pty_size": {"cols": 40, "rows": 30}
      }
    ]
  }
}
```

A stale precondition fails before mutation:

```json
{
  "protocol_version": 1,
  "request_id": "r18",
  "instance_id": "91cbcc1a-2a1f-4f87-930b-3afc6aec2901",
  "revision": "42",
  "ok": false,
  "error": {
    "code": "precondition_failed",
    "message": "The tab layout changed since the supplied observation.",
    "effect": "none",
    "details": {"actual_layout_token": "layout:tab:t2:8"}
  }
}
```

Streams start with `type: state`, the full projection and its revision.
Subsequent batches use typed changes:

```json
{
  "protocol_version": 1,
  "instance_id": "91cbcc1a-2a1f-4f87-930b-3afc6aec2901",
  "revision": "43",
  "type": "change",
  "changes": [
    {"kind": "settings_changed", "window_id": "w1", "generation": "4"}
  ]
}
```

Changes carry new field values or explicit removals. Structural batches include
affected parent topology and selection at the same revision, preventing dangling
references.

Use UTF-8 JSON without an output BOM; streams emit one object per line. Encode
64-bit counters/generations as decimal strings, IDs/cursors as strings, finite
geometry as numbers, and times as UTC timestamps. Schema-defined null means
meaningful absence; unobservable data has availability/reason. Client transport
errors use the same shape, with unknown instance/revision set to null.

Reject unknown request fields, invalid arguments, and unsupported protocol
versions before mutation. Optional response fields, advertised operations, and
event kinds can be additive: clients ignore unknown response fields and
resynchronize on unsupported changes. Changed field meanings/operation semantics
require a protocol-version change. Version saved-state and capture schemas too.

### Text and keys

`send-text --file PATH` and `--file -` read UTF-8 files/stdin. Strip one leading
UTF-8 BOM, reject invalid encoding, and preserve Unicode normalization, embedded
characters, and CR/LF without appending a newline. Ghostty paste encoding may add
framing or documented normalization; test those transformations and report
encoded-byte progress.

Structured input uses UTF-8 JSON with the same BOM handling. JSON escapes decode
to literal characters; mightty does not parse text as shell commands. Enforce
source byte/step limits while reading, before submission.

Canonical keys are named lowercase values such as `enter`, `escape`, and
`left`, or a single Unicode scalar, with explicit `ctrl`, `alt`, `shift`,
and `super` modifiers. CLI aliases such as `Enter` normalize. Default
`event: tap` sends press/release through the selected path; bounded balanced
sequences also support press/repeat/release. Use text steps for composed text,
without inferring a physical keyboard layout from printable characters.
