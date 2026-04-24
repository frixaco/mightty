# Windows ConPTY Hardening Plan

This plan covers the remaining Windows/ConPTY work before treating Windows as the stable baseline backend for future Unix PTY support.

## Status

Completed in this pass.

- Added platform-neutral `PtySession`, `PtyError`, `PtySize`, and `PtyRead` shell API types.
- Kept temporary `ConPtyShell` and `ConPtyError` aliases for compatibility.
- Replaced `peek() + read()` usage with `try_read()` returning `Data`, `WouldBlock`, or `Eof`.
- Added explicit process exit detection with `WaitForSingleObject`.
- Hardened writes to loop until all input bytes are written.
- Disabled broad child handle inheritance.
- Set invalid std handles on the child startup info so hosted console output stays attached to ConPTY instead of leaking to the parent process.
- Added behavioral tests for spawn, output, exit, resize, large writes, and invalid dimensions.
- Verified with `cargo fmt`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Goal

Make the Windows shell bridge reliable enough to serve as the reference implementation for a later platform-neutral PTY API.

The Windows path now builds, spawns `cmd.exe` in tests, wires input/output into `TerminalWidget`, supports resize, reports process exit, and passes formatting, check, clippy, and tests. The step-by-step plan below is retained as an implementation record and regression checklist.

## Step 1: Add a Process Exit Signal

Problem:

`PeekNamedPipe` returning no bytes is currently treated as "no output right now." A closed pipe or exited process can become indistinguishable from idle output in the widget loop.

Tasks:

1. Add an explicit process status API to the Windows shell bridge.

   Suggested shape:

   ```rust
   pub enum PtyRead {
       Data(usize),
       WouldBlock,
       Eof,
   }
   ```

   Or, if keeping the current API temporarily:

   ```rust
   pub fn has_exited(&self) -> Result<bool, ConPtyError>;
   ```

2. Use `WaitForSingleObject(process_handle, 0)` to detect process exit without blocking.
3. Optionally use `GetExitCodeProcess` for diagnostics.
4. Update the widget I/O loop so shell exit sets `exit_flag` and returns.
5. Make EOF distinct from "no bytes available."

Acceptance criteria:

- Running `exit` in the shell marks the pane as exited.
- With more than one pane, the exited pane is removed.
- With one pane, the app does not spin or hang an I/O thread forever.

## Step 2: Tighten Handle Inheritance

Problem:

The anonymous pipes are created inheritable, and `CreateProcessW` is called with handle inheritance enabled. That can leak app-owned pipe handles into the child process or descendants, which can keep pipes open unexpectedly and delay EOF.

Tasks:

1. Audit every handle created in `src/shell/windows.rs`.
2. Ensure only handles that must be inherited are inheritable.
3. Prefer passing the pseudoconsole through `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` and disabling broad handle inheritance.
4. If any inherited handles are truly needed, use an explicit handle list rather than blanket inheritance.
5. Verify all failure paths still close owned handles exactly once.

Acceptance criteria:

- The shell process does not inherit the parent-side pipe handles.
- Shell exit reliably produces EOF or process-exit detection.
- No double-close or leaked-handle paths remain in spawn failure cases.

## Step 3: Handle Partial Writes

Problem:

`WriteFile` can report fewer bytes written than requested. The current code records `bytes_written` but does not loop until all bytes are sent.

Tasks:

1. Change `ConPtyShell::write` to loop until the full input slice is written.
2. Treat `bytes_written == 0` as an error to avoid an infinite loop.
3. Preserve the existing broken-pipe behavior as a clean shell-exit path where appropriate.
4. Add a focused test using a large write or paste-sized input.

Acceptance criteria:

- Large paste/input payloads are fully written.
- Partial write cannot silently drop bytes.
- Zero-progress writes fail instead of looping.

## Step 4: Make Read Semantics Explicit

Problem:

The current `peek() + read()` pair works for the Windows polling loop, but it is easy to misuse and will not map cleanly to Unix.

Tasks:

1. Replace or wrap `peek() + read()` with a single nonblocking read method.

   Suggested temporary Windows shape:

   ```rust
   pub fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, ConPtyError>;
   ```

   Longer-term platform-neutral shape:

   ```rust
   pub enum PtyRead {
       Data(usize),
       WouldBlock,
       Eof,
   }
   ```

2. Internally keep using `PeekNamedPipe` before `ReadFile` on Windows.
3. Return a distinct EOF state when the pipe is closed or the process has exited.
4. Update the widget loop to consume the new method.

Acceptance criteria:

- Idle shell output does not block the I/O thread.
- Shell exit is not represented as idle output.
- The resulting API can be mirrored by Unix nonblocking PTY reads later.

## Step 5: Improve Shutdown Behavior

Problem:

`shutdown` and `Drop` currently call `TerminateProcess`. That is acceptable for forced teardown, but interactive terminal close should prefer graceful shutdown where possible.

Tasks:

1. Decide the intended close behavior:
   - Force-kill on pane close.
   - Gracefully request shell exit, then kill after timeout.
2. If graceful close is desired, close the PTY input pipe first and wait briefly.
3. Use `WaitForSingleObject` with a short timeout before `TerminateProcess`.
4. Keep forced termination as the final fallback.
5. Ensure `Drop` remains non-panicking and idempotent.

Acceptance criteria:

- Pane close cleans up shell resources.
- A hung shell cannot block app shutdown indefinitely.
- Normal shell exit does not require forced termination.

## Step 6: Add Behavioral ConPTY Tests

Problem:

Current tests only cover spawn and invalid dimensions. They do not prove the bridge actually transports bytes or reports lifecycle events.

Tasks:

1. Add a test that spawns `cmd.exe /d /q`.
2. Write a simple command, such as:

   ```text
   echo mightty-ready
   ```

3. Poll output until the marker appears or a timeout expires.
4. Add a test for process exit:

   ```text
   exit
   ```

   Verify the shell reports EOF or exited state.

5. Add a resize smoke test that calls `resize` with valid dimensions.
6. Add a large-write test that sends paste-sized input and verifies no write error.
7. Keep tests timeout-bounded so failures do not hang CI.

Acceptance criteria:

- Tests prove spawn, output read, input write, resize, and exit detection.
- Tests fail quickly on broken ConPTY behavior.
- `cargo test` remains usable during normal development.

## Step 7: Add Diagnostics Without Noisy Logs

Problem:

Most shell bridge errors are currently collapsed into simple `eprintln!` messages in the widget thread.

Tasks:

1. Preserve Windows error codes where available.
2. Include operation context in errors:
   - create pipe
   - create pseudoconsole
   - create process
   - read
   - write
   - resize
   - shutdown
3. Keep UI-thread logging minimal.
4. Consider storing the last shell error in `TerminalWidget` for feedback captures.

Acceptance criteria:

- Spawn/read/write/resize failures are diagnosable from logs.
- Feedback captures can eventually include shell bridge failure context.

## Step 8: Prepare for the Unified PTY API

Problem:

The current exported type is named `ConPtyShell`, which leaks the Windows backend into higher-level code.

Tasks:

1. After hardening the Windows implementation, introduce platform-neutral names:

   ```rust
   PtySession
   PtyError
   PtySize
   PtyRead
   ```

2. Keep temporary compatibility aliases if needed:

   ```rust
   pub type ConPtyShell = PtySession;
   pub type ConPtyError = PtyError;
   ```

3. Update `TerminalWidget` to depend only on `PtySession`.
4. Keep `src/shell/windows.rs` as the Windows backend implementation.
5. Leave `src/shell/unix.rs` as unsupported until the Unix session is implemented.

Acceptance criteria:

- App code no longer depends on ConPTY-specific naming.
- Windows behavior is unchanged after the rename.
- Unix work can start by implementing the same `PtySession` contract.

## Suggested Order

1. Process exit signal.
2. Read semantics.
3. Handle inheritance.
4. Partial writes.
5. Behavioral tests.
6. Shutdown behavior.
7. Diagnostics.
8. Unified PTY API rename.

This order reduces risk first: exit detection and read semantics affect day-to-day correctness, handle inheritance can affect EOF behavior, and tests should lock those fixes down before the API rename.
