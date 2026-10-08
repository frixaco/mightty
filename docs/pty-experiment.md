# portable-pty experiment

Tested on 2026-10-03 against mightty revision
`5d83306eb9960a221ab830840b878bedcd9dd04b`, Windows build 26300,
PowerShell 7.6.6, and published `portable-pty` 0.9.0.

## Decision

portable-pty works with our existing Ghostty terminal, worker, and real GPUI
application. It offers simpler process setup and could remove a substantial
part of our OS-specific spawning code. Windows cancellation and draining during
shutdown worked without changing the worker or accessing the library's private
pipe handles.

The strongest terminal-support improvement came from the newer ConPTY engine
that the crate loaded, rather than from replacing the Rust wrapper alone.
With the system engine, portable-pty did not improve mouse-mode or Kitty graphics
forwarding. The stock crate also changes working-directory semantics and has a
reproducible Windows child-killer error-reporting defect.

This leaves a useful candidate for a later migration: portable-pty for new
sessions, a deliberate ConPTY engine choice, explicit launch-policy checks, and
our native Windows handoff path. The experiment does not migrate production:
the feature is optional and native remains the default.

## Implemented comparison

`portable-pty-experiment` enables `src/shell/experiment.rs`. It adapts the
library to the existing `PtyParts`, input, output, and control interface. The
widget, key encoder, queues, backpressure, write/resize acknowledgements, and
renderer are unchanged. Handoffs always use our native implementation.

The runner exercises these configurations in separate processes:

| Selector | Wrapper | ConPTY engine | Creation flags |
| --- | --- | --- | --- |
| `native` | Existing mightty code | Windows system | `0` |
| `native-flags` | Existing mightty code | Windows system | `0x7`, matching the crate |
| `portable` | portable-pty 0.9.0 | Crate's normal DLL selection | `0x7` |
| `portable-system` | portable-pty 0.9.0 | System fallback, verified by loaded modules | `0x7` |
| `native-passthrough` | Existing mightty code | Windows system | `0xf`, separate follow-up |

The crate searches for `conpty.dll` before falling back to `kernel32.dll`.
On this machine it found
`C:\Users\frixa\AppData\Local\Programs\Tern\conpty.dll`, version
`1.24.2607.10001`. The example records the DLL actually loaded, not just the
requested selector. For `portable-system`, the PowerShell runner removes PATH
directories containing `conpty.dll` from that disposable process's environment.
The selector alone does not force the crate's loader to use the system engine.

## Results and coverage

The release matrix ran **290 checks: 271 passed, 19 failed**, with no watchdog
timeouts. Core scenarios were repeated three times with reversed backend order
on the second round. Counts include repeated checks of known compatibility
differences; failures are not hidden or treated as passes.

| Configuration | Checks | Failures |
| --- | ---: | ---: |
| Native | 71 | 0 |
| Native with matching flags | 71 | 0 |
| portable-pty with bundled engine | 74 | 10 |
| portable-pty with system engine | 74 | 9 |

The successful scenarios cover:

- `cmd.exe`, Windows PowerShell, and PowerShell 7; nonzero exit status and final output.
- An executable path with spaces; argument quoting; Unicode environment and cwd;
  environment overrides, removal, and clearing.
- Unicode, emoji, combining marks, ANSI colors, cursor addressing, and OSC titles.
- Literal input and Ghostty-encoded Up, F5, Ctrl+A, Alt+X, and Shift+Tab.
- A separate 12/12 follow-up verified inbound `é界🦀` through Ghostty's text
  encoder and the shell's UTF-16 key events across all four configurations.
- 65,536 input characters; 32,768 output lines, approximately 2.47 MiB of source text.
- PTY resize acknowledgements and 101 rapid resizes with the final size checked in the shell.
- Closing idle sessions, blocked input, and blocked output; attached child-process termination.
- Four simultaneous sessions and repeated create/close cycles without handle growth.
- Actual GUI keyboard editing, tabs, splits, divider dragging, quick terminal, settings reload,
  stale-target rejection, process diagnostics, snapshots, and orderly shutdown.
- PowerShell editing and Neovim modal editing/saving through real GPUI input.
- A 30-second GUI output stress run with three producers, captures, input, and resizing.

Both native and portable pane captures were visually inspected: the Unicode,
wide characters, combining marks, colors, and search overlay rendered alike.
The initial debug output-stress runs exceeded the snapshot watchdog on both
backends; **all four release configurations passed** that test.

### Terminal protocol support

These probes report the state observed in Ghostty after bytes passed through the
real shell and ConPTY. A probe completing successfully does not mean every
probed protocol was forwarded.

| Protocol | Native | Native `0x7` | Portable + system | Portable + bundled |
| --- | --- | --- | --- | --- |
| Alternate screen | Yes | Yes | Yes | Yes |
| Bracketed paste mode | Yes | Yes | Yes | Yes |
| Synchronized output | Yes | Yes | Yes | Yes |
| Mouse tracking mode | No | No | No | Yes |
| OSC 7 working directory | Yes | Yes | Yes | Yes |
| OSC 8 hyperlinks | Yes | Yes | Yes | Yes |
| RGB color `18,52,86` | Preserved | Preserved | Preserved | Preserved |
| Kitty direct graphics | Stripped | Stripped | Stripped | Received: 1×2 image, 6 decoded bytes |

The bundled engine forwarded the original mouse-mode and Kitty APC sequences.
The system engine dropped them, including when the native bridge used matching
flags. Adding the passthrough bit (`0xf`) also failed to recover these protocols
on this Windows installation; its five follow-up scenarios passed on all three
rounds but showed the same unsupported protocol states.

### Compatibility differences and the failures

1. **No requested cwd:** portable-pty starts in `USERPROFILE`; native inherits the
   application cwd. The two portable configurations failed this contract check
   on all three rounds, accounting for six failures.
2. **Nonexistent requested cwd:** portable-pty silently falls back to home;
   native returns a launch error. Both portable configurations failed this
   check on all three rounds: another six failures. The adapter deliberately
   leaves these semantics visible. A migration should validate the requested
   directory and explicitly set the fallback cwd.
3. **`clone_killer().kill()` result:** 0.9.0 successfully terminated the process
   but returned an error because its Windows success check is inverted. This
   reproduced with both engines on all three rounds: six failures. Upstream
   fixes that check; our adapter uses the direct child kill path and does not
   patch the crate. A migration needs an appropriate fixed dependency or patch.
4. **Synthetic initial title:** the bundled engine did not synthesize an OSC
   shell title, so the existing full control smoke test's `title.observed_at`
   requirement failed once. Its captured state confirms correct settings and
   a visible cursor, with `title.reported` and `title.observed_at` both null.
   A follow-up using an explicit OSC title passed the entire control smoke test.
   This is a fixture expectation about title emission, not loss of the title
   callback. `-ExplicitTitle` makes that expectation explicit without changing
   the original smoke-test default.

### Performance

The separate release performance run ran **48/48 successful checks** without
concurrent compiler work. These are local scenario timings, not broad terminal
benchmarks. Spawn medians use 36 create calls per configuration; the other
medians use three runs. Bulk-output time includes shell startup and completion.

| Metric | Native | Native `0x7` | Portable + system | Portable + bundled |
| --- | ---: | ---: | ---: | ---: |
| PTY/process creation median | 10.48 ms | 10.70 ms | 9.71 ms | 8.95 ms |
| 2.47 MiB output scenario | 789 ms | 788 ms | 907 ms | 759 ms |
| 101 resizes, including Ghostty resize | 4.88 ms | 4.92 ms | 4.91 ms | 6.08 ms |
| Idle shutdown | 9.20 ms | 9.12 ms | 3.12 ms | 3.08 ms |

The native creation path also creates a temporary pseudoconsole to check
availability; the library does not. The small creation advantage should not
be attributed entirely to a faster implementation. All configurations had
stable process-handle counts across their 36 measured create/close cycles.

In the GUI stress run, the maximum individual control call was 384 ms native,
378 ms portable/bundled, 393 ms native/matching flags, and 401 ms portable/system.
The first producer wrote about 291 MB under native versus 85 MB under the
bundled engine. That is not an equal-work throughput comparison: the engines
transform/coalesce/forward the VT stream differently. It does show that richer
forwarding did not produce a universal performance win.

## Complexity and ergonomics

`CommandBuilder`, `openpty`, `spawn_command`, `try_clone_reader`, `take_writer`,
and `resize` are a much smaller surface to maintain than process attribute
lists, pipe allocation, Windows argument quoting, and environment-block
construction. Around 300 physical lines of the current Windows spawn/setup and
argument/environment helpers become candidates for deletion. That is a gross
displaced-code estimate, not a measured net migration reduction.

The comparison adapter is about 320 physical lines, including native dispatch,
handoff forwarding, existing acknowledgement semantics, error conversion,
process watching, and shutdown policy. The prototype retains both implementations
and therefore adds code. No code deletion or production simplification is claimed.

What a migration would still own:

- Windows default-terminal COM handoff, adopted handles, and its resize signaling.
- The worker's two threads, bounded queues, output draining, cancellation, and UI scheduling.
- Launch policy, process diagnostics, shutdown policy, and acknowledgements.
- A deliberate and reproducible choice of the ConPTY engine and its distribution.

Windows `CancelSynchronousIo` on the worker thread still interrupts the library's
synchronous streams. The stalled-I/O experiments resolved the earlier concern
that private pipe handles might prevent us from retaining cancellable shutdown.

Seven new packages were added to the lockfile: portable-pty, cfg_aliases 0.1.1,
nix 0.28, serial2, shared_library, shell-words, and winreg 0.10.1. They are behind
the experimental feature. Unix remains on our existing implementation;
its potential simplification is based on source inspection, not a Unix runtime
test. No usable Unix development environment was available here.

## Reproduce or try the GUI

From the repository root in PowerShell:

```powershell
mise exec -- cargo build --release --features portable-pty-experiment --bin mightty --example pty_experiment
pwsh -NoProfile -File tools/pty-experiment.ps1 -Executable target/release/examples/pty_experiment.exe -GuiExecutable target/release/mightty.exe
```

The runner saves JSON, summaries, stdout/stderr, and watchdog outcomes under
`artifacts/pty-experiment/`. Its default run includes checks that intentionally
expose the compatibility failures, so a nonzero exit is expected with 0.9.0.
`-SkipGui`, `-Cases`, `-Backends`, `-GuiModes`, and `-Repetitions` allow focused
reruns. Never rebuild the executable while a comparison is running: Windows
locks executable files in use.

The saved 290-check matrix predates the Unicode-input and explicit-title
follow-ups; the current runner's defaults include those additional checks.

To play with a disposable real GUI using portable-pty:

```powershell
$env:MIGHTTY_PTY_BACKEND = 'portable'
mise exec -- cargo run --features portable-pty-experiment -- --test-instance --data-dir "$PWD/artifacts/pty-experiment/manual"
```

Set `MIGHTTY_PTY_BACKEND` to `native` for the comparison. The optional feature
defaults to native when the variable is absent. These test instances use
isolated settings/storage and disable global hotkeys and the default-terminal
COM server.

## Evidence and verification

- [Release matrix](../artifacts/pty-experiment/release/results.json)
- [Quiet performance measurements](../artifacts/pty-experiment/performance/results.json)
- [Performance summary](../artifacts/pty-experiment/performance/summary.json)
- [Passthrough follow-up](../artifacts/pty-experiment/passthrough/results.json)
- [Title-policy follow-up](../artifacts/pty-experiment/title-policy/results.json)
- [Title state](../artifacts/pty-experiment/title-policy/provenance.json)
- [Unicode input follow-up](../artifacts/pty-experiment/unicode-input/results.json)
- [GUI stress observations](../artifacts/pty-experiment/release/stress-summary.json)
- [Native pane PNG](../artifacts/pty-experiment/release/visual-native.png)
- [Portable pane PNG](../artifacts/pty-experiment/release/visual-portable.png)

Formatting and clippy passed for the normal and experimental configurations.
The normal configuration, experimental/native configuration, and
experimental/portable configuration each passed 157 unit tests plus one
doctest; one existing explicit GPU test remained ignored. Handoff tests passed
through the retained native path. This did not test system-wide default-terminal
registration or a real external COM activation.

Primary upstream references:
[API](https://docs.rs/crate/portable-pty/0.9.0/source/src/lib.rs),
[command builder](https://docs.rs/crate/portable-pty/0.9.0/source/src/cmdbuilder.rs),
[ConPTY loader/flags](https://docs.rs/crate/portable-pty/0.9.0/source/src/win/psuedocon.rs),
[published child implementation](https://docs.rs/crate/portable-pty/0.9.0/source/src/win/mod.rs),
[upstream child implementation](https://github.com/wezterm/wezterm/blob/main/pty/src/win/mod.rs).
