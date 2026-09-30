**mightty daily-use opportunities — 2026-09-30**

Reviewed commit `891bd0729c3d990eba2c9a1031dcfe1733522b21`. These five opportunities are ranked by impact, frequency, confidence, and implementation cost. No implementation, dependency, generated-binding, or tracked-documentation changes were made.

The architecture is coherent: `main.rs` owns windows and Windows application services; `PaneContainer` owns actions, tabs, settings, and workspace restoration; `Split` owns pane topology; `TerminalWidget` connects GPUI interaction to a wake-driven PTY worker and the safe Ghostty API. Platform shell modules own process resources. Ghostty terminal, input, selection, search, and graphics objects stay on the foreground thread, with ownership and lending borrows contained inside `src/ghostty/`. The recommendations below preserve those boundaries.

Documentation was checked against implementation before selecting leads. The README's search, mouse reporting, paste protection, settings, action model, workspace restoration, shell metadata, quick terminal, handoff, and graphics features all have corresponding implementation paths. They are not proposed as missing. `PRD.md:59-63` still describes directory-derived titles and equal-flex splits; current titles follow active-pane metadata (`src/pane_container.rs:231-263`) and the split tree stores ratios (`src/split.rs:57-68`). The roadmap acknowledges implemented areas at `docs/feature-roadmap.md:7-19`, although its search discussion at lines 95-100 retains future-work wording: the actual search boundary exists in `src/ghostty/search.rs` and `tools/ghostty-search/src/search.zig`. Presence of code and tests is distinguished from verified end-to-end behavior throughout this report.

| Rank | Opportunity | Confidence | Rough effort |
| --- | --- | --- | --- |
| 1 | Make pane shutdown interrupt blocked input | High; worker failure reproduced | 2–4 days |
| 2 | Remove synchronous profile discovery from settings polling | High; call latency measured | 1–2 days |
| 3 | Derive terminal cell geometry from the configured font | High for mismatch; visual effects inferred | 1–2 days |
| 4 | Give installed packages a stable update feed | High for published URL topology; installation unverified | 1–2 days plus release verification |
| 5 | Connect terminal text input to native composition | High for absent integration; IME behavior unverified live | 3–7 days |

**1. Make pane shutdown interrupt blocked input**

Category: PTY reliability and responsiveness. Confidence: high.

**Who benefits and current outcome.** Users closing a pane after a large paste, or after its input consumer stalls, need the rest of their terminals to remain usable. Today shutdown queues behind that write and synchronously joins the worker. A stalled session can therefore stall the whole application during pane destruction. This ranks first because recovery from a stuck pane must not require losing unrelated sessions.

**Evidence and safeguards.**

- `src/widget/mod.rs:1320-1329` encodes a paste through Ghostty and submits its bytes as one `Write`; `src/widget/mod.rs:928-939` uses the same command path for keys. Terminal replies also enter that FIFO (`src/widget/mod.rs:296-303`). These preserve input ordering.
- `src/widget/pty.rs:63-86` puts writes, resize, and shutdown on one unbounded command queue. The control thread executes `input.write_all()` synchronously before it can receive another command.
- `src/shell/windows.rs:479-482,675-708` uses synchronous `WriteFile`, with no cancellation or deadline. Unix also loops in a blocking write (`src/shell/unix.rs:171-206`), so the solution should remain behind the shared shell interface.
- `src/widget/pty.rs:128-137` enqueues `Shutdown` and then joins the control and reader threads. `TerminalWidget::drop` calls it synchronously after cancelling output consumption (`src/widget/mod.rs:1355-1363`). Closing a tab removes its entities through `src/pane_container.rs:534-558`; replacing a workspace also releases old tabs at line 501. The application-wide freeze is an inference from this foreground destruction path, not a measured GPUI freeze.
- There are real safeguards: output has a bounded queue and separate reader (`src/widget/pty.rs:64,89-112`); shell handles have explicit ownership; shutdown has a 250 ms process-wait policy. However, that policy is reached only after the blocked write completes (`src/shell/windows.rs:582-593,616-645`). Handed-off processes are deliberately not terminated (`src/shell/windows.rs:589,614-615`); preserve that guarantee.

**Reproduction performed.** An external Rust harness compiled the unchanged `profile.rs`, `shell/mod.rs`, and `widget/pty.rs` via `#[path]`, without Ghostty or GPUI. On Windows 11 Pro build 26300, Rust/Cargo 1.97.1 and PowerShell 7.6.6:

1. Launched real ConPTY PowerShell with `-NoLogo -NoProfile -NonInteractive -Command "[Console]::WriteLine('REVIEW_READY'); Start-Sleep -Seconds 6"`.
2. Waited for its ready marker, queued a 16 MiB write, waited 100 ms, dropped the output receiver to model widget teardown, and called worker shutdown. Shutdown took **18,930 ms**. This is one large-input observation, not a typical-paste benchmark.
3. Exercised the handoff pipe path with a 1 MiB write and an input peer that stayed open without reading. Shutdown **did not complete within one second**. Closing the owned input/output peers released it; completion arrived at **1,002 ms** from shutdown start. This establishes dependence on peer progress, rather than a bounded shutdown.

The 13 existing Windows shell tests and three profile tests all passed in this harness, including input after idle, ordered output, resize, process exit, paste-sized writes, and handoff ownership. Those successful cases do not cover cancellation of an in-progress write.

**Smallest useful change and benefit.** Add a platform-owned cancellation mechanism for pending input, exposed safely through `src/shell/`. Make shutdown request cancellation immediately, independently of the write FIFO, before waiting for worker completion. Keep normal writes ordered and keep output drainage/resource cleanup explicit. Separating the writer from control is justified only if necessary to make cancellation and resize independent. Expected benefit: a stuck pane can be closed without freezing other panes. Main tradeoff: shutdown intentionally abandons pending input, and cancellation races need careful ownership handling; merely detaching threads would leave the resource problem unresolved.

**Focused success check.** Repeat both probes with a one-second completion bound; close the corresponding pane while another pane receives keystrokes. Exercise shutdown during simultaneous output and input, retain the existing ordered-write tests, and repeat create/close cycles while checking owned process/handle cleanup. Keep the handoff test proving that mightty does not terminate the supplied client process.

**2. Remove synchronous profile discovery from settings polling**

Category: responsiveness and simpler application state. Confidence: high.

**Who benefits and current outcome.** Windows users with `wsl.exe` on PATH pay for WSL discovery throughout an otherwise idle session. Typing, rendering, and quick-terminal focus handling share the foreground thread with these calls. An unchanged settings file still causes process launch and a synchronous wait; a slow WSL service has no timeout here.

**Evidence and safeguards.**

- `src/settings.rs:284-298` reads the file and calls `discover_profiles()` **before** checking whether anything changed. `src/settings.rs:799-818` launches `wsl.exe --list --quiet` with `Command::output()` and waits for completion.
- Every pane container invokes this inside its foreground entity update every two seconds (`src/pane_container.rs:586-617`). Windows additionally invokes it inside the foreground application update every 500 ms (`src/main.rs:327-339,408-415`), even with quick terminal disabled.
- Each window creates a separate store (`src/main.rs:77`); the Windows controller creates another (`src/main.rs:269`). Nominally this means two discovery attempts per second from the controller plus one every two seconds per window. Actual frequency includes the time spent waiting in each call.
- Atomic settings publication is already implemented: resolution succeeds before `current` is replaced; invalid reloads retain the previous generation (`src/settings.rs:296-318`). Existing panes receive updated action bindings separately (`src/pane_container.rs:596-604`). Preserve these safeguards.

**Measurement performed.** The external Rust probe used the same `Command::new(wsl).args(["--list", "--quiet"]).creation_flags(0x08000000).output()` sequence. Ten sequential calls returned success and 32 bytes each; elapsed times were **55, 50, 49, 49, 50, 49, 50, 52, 50, 50 ms**. This measures the subprocess call, not complete reload duration or UI frame time. Because the traced callers execute it synchronously on the foreground thread, those callers must wait at least that long in this environment. No frame-rate or idle-CPU improvement is claimed.

**Smallest useful change and benefit.** Keep file-change checks lightweight; cache discovered profiles and refresh them in one background job with a timeout. Publish the completed discovery snapshot and resolved settings on the foreground thread. Start by removing discovery from every polling tick; a slower bounded discovery refresh can still detect distributions being added or removed. Reuse one application discovery result across stores rather than maintaining competing process polls. Expected benefit: remove this recurring foreground wait and make a stalled WSL command recoverable. Main tradeoff: newly installed profiles may appear after a short refresh delay.

**Focused success check.** Instrument discovery calls during an unchanged-settings idle session with normal and quick windows open: the 500 ms and two-second polls should launch no discovery subprocesses. Make a controlled discovery process delay or fail and verify foreground interaction continues, the refresh times out, and the last valid settings remain active. Then add/remove a profile and verify the palette updates after the bounded refresh.

**3. Derive terminal cell geometry from the configured font**

Category: rendering correctness and accessibility. Confidence: high for the geometry mismatch; clipping and pointer misalignment are inferred.

**Who benefits and current outcome.** Users increasing text size for readability, or choosing another monospace font, need terminal columns, cursor placement, and selection to match the text. The setting changes the glyph font/size, but the terminal continues laying out and hit-testing a fixed grid. The feature exists; its geometry is incomplete.

**Evidence and safeguards.**

- Settings accept a nonempty font family and sizes from 6 through 96 px (`src/settings.rs:450-462`). New panes receive that resolved configuration (`src/pane_container.rs:309-315,352-360`), consistent with the documented new-pane reload policy.
- `src/widget/mod.rs:376` initializes `cell_size` to **9.6 × 19.2 px**. Searching all its uses found no assignment that derives it from configured font metrics.
- The renderer uses the configured font and size (`src/widget/render.rs:140-144,302-304`) while positioning and clipping segments using those fixed cell dimensions (`src/widget/render.rs:278-310`). Thus changing glyph size does not change segment allocation or line height.
- The same fixed geometry determines PTY dimensions (`src/widget/mod.rs:738-755`), cursor position/size (`src/widget/render.rs:368-390`), search highlight bounds (`src/widget/render.rs:329-340`), mouse reporting geometry (`src/widget/mod.rs:1181-1203`), and local selection hit-testing (`src/widget/mod.rs:1335-1350`). This is one shared geometry defect, not several independent feature gaps.
- Ghostty already supplies cell width and wide-cell information; the renderer respects it and isolates non-ASCII/multi-column segments (`src/widget/render.rs:41-49,928-929`). Keep Ghostty in charge of Unicode column semantics.

**Validation performed.** Static tracing plus evaluation of the current dimension formula gives **83 columns × 31 rows** for an 800 × 600 px terminal area. The calculation is identical for configured font sizes 16, 32, or 96 because font configuration never reaches that formula. I did not run the complete renderer or obtain feedback captures; exact glyph clipping, fallback metrics, and mixed-DPI output remain unverified.

**Smallest useful change and benefit.** Resolve the primary font through GPUI's text system and derive logical cell advance and line height from that font at the configured size. Store the resulting geometry once per relevant font/scale change and use it in the existing render, resize, selection, mouse, and graphics paths. Trigger resize when geometry changes even if window bounds do not. This does not require a renderer rewrite or per-glyph terminal-width calculation. Expected benefit: larger and alternative fonts become usable without cursor/selection drift or clipped segments. Main tradeoff: font fallback and rounding need a consistent policy; simply multiplying the current constants by font size fixes only one family.

**Focused success check.** Render a known ASCII/wide/combining-character fixture at 16 and 32 px and with a second installed monospace family. Verify PTY dimensions decrease appropriately as text grows, cursor and selection agree with the grid, and feedback JSON cell columns correspond to pixel placements. Move between different-DPI displays and verify recalculation uses logical units consistently.

**4. Give installed packages a stable update feed**

Category: shipping and maintenance. Confidence: high for the release configuration; end-to-end update installation unverified.

**Who benefits and current outcome.** Users installing one release through AppInstaller should receive later fixes without manually finding a new installer. The current workflow creates update settings, but each installed release points back to its own tag's manifest. Publishing the next tag does not advance the old installation's feed.

**Evidence and safeguards.**

- `tools/package-windows.ps1:99-102` defaults the base URI to `/releases/download/v$version`. Lines 210-221 derive both the package URI and the AppInstaller self URI from that base.
- `.github/workflows/release-windows.yml:69-74` explicitly passes the current tag's download directory; lines 101-104 upload artifacts to that release. No step updates a shared feed or previous releases' manifests.
- `packaging/windows/Mightty.appinstaller.in:2-17` sets the root `Uri`, a versioned main package, and on-launch/background checks. Windows uses the AppInstaller URI to check for newer manifests, as documented in [Microsoft's update contract](https://learn.microsoft.com/en-us/windows/msix/app-installer/auto-update-and-repair--overview). The inference is that the shipped workflow leaves v0.1 clients checking the v0.1 manifest after v0.2 is published.
- The release verifier checks identity, versions, signature, and URI filenames (`tools/verify-windows-release.ps1:190-217`), but it does not check feed continuity across releases. The upgrade test explicitly installs both MSIX files with `Add-AppxPackage` (`tools/test-windows-package-upgrade.ps1:33,40`), so it verifies data preservation rather than update discovery.
- Signing, timestamping, payload verification, publisher matching, and removing the private certificate before artifact verification are already present. Preserve those controls.

**Validation performed.** Rendered the unchanged AppInstaller template in memory using the script's URI construction for versions 0.1.0 and 0.2.0. Their self URIs were respectively:

```text
https://github.com/frixaco/mightty/releases/download/v0.1.0/mightty.appinstaller
https://github.com/frixaco/mightty/releases/download/v0.2.0/mightty.appinstaller
```

Each main-package URI likewise referenced only its own version. No package was signed, installed, or published; Windows update discovery itself was not exercised.

**Smallest useful change and benefit.** Separate the stable AppInstaller feed URI from versioned package download URIs. Publish the verified package first, then advance an architecture-specific stable manifest to its new version. Check that two generated releases retain the same feed URI while their package version/URI advance. Expected benefit: the existing Windows updater can discover later fixes without a custom updater service. Main tradeoff: the stable feed needs reliable publication ordering and an explicit policy for existing tag-specific installations. Fix this before expanding public distribution.

**Focused success check.** On a clean Windows test user or VM, install vN **through its AppInstaller file**, publish vN+1 to the same feed, and request/check update availability without manually supplying vN+1's MSIX. Verify successful update and settings/workspace preservation. Also verify a failed publication leaves the prior valid feed available.

**5. Connect terminal text input to native composition**

Category: input compatibility and everyday workflows. Confidence: high for the missing adapter; live IME outcomes unverified.

**Who benefits and current outcome.** Users composing Chinese, Japanese, or Korean text need to enter filenames, commands, and TUI text through the Windows IME. Users also need composed text in search and palette queries. mightty forwards key events and Unicode `key_char`, but does not register the native text-input handler that GPUI uses for composition and committed text. Clipboard paste is already implemented as a separate route; it does not solve interactive composition.

**Evidence and existing capabilities.**

- The terminal registers key-down/up handlers and a focus handle (`src/widget/render.rs:604-618`). Its canvas paint callback is empty (`src/widget/render.rs:682-688`). `rg -n 'set_input_handler|handle_input|InputHandler|EntityInputHandler' src` returned no matches.
- `src/widget/input.rs:9-36` turns every event into Ghostty input and explicitly calls `set_composing(false)`. The safe wrapper already exposes composition and owned UTF-8 setters (`src/ghostty/key.rs:115-130`); no generated-binding change is necessary for those operations.
- Search appends `key_char` directly to a string and pops it on Backspace (`src/widget/mod.rs:777-814`). The palette follows the same pattern (`src/pane_container.rs:825-879`). Neither receives native composition commits through a text editor.
- The locked dependency is GPUI 0.2.2. Its local source documents `Window::handle_input` as the paint-phase registration for IME/text input (`gpui-0.2.2/src/window.rs:3392-3413`). Windows composition routes both marked text and `GCS_RESULTSTR` through that handler (`gpui-0.2.2/src/platform/windows/events.rs:664-690`); its helper returns without a handler at lines 1260-1267. This traces the missing delivery boundary rather than assuming that all Unicode input fails.
- GPUI already recognizes AltGr and excludes its synthesized Ctrl/Alt modifiers on applicable layouts (`gpui-0.2.2/src/platform/windows/events.rs:1471-1485`). I am not proposing to duplicate that logic or claiming every international layout is broken.

**Validation performed.** Inspected the actual cached GPUI source used by `Cargo.lock` and the full mightty input registration path. No live Windows IME session, candidate-window positioning, dead-key sequence, or TUI composition test was performed because the full app could not be built in this checkout. Dropped or incorrect composed input is the expected result of the missing adapter, not a claimed observed screen behavior.

**Smallest useful change and benefit.** Implement the terminal's GPUI input handler with local preedit state and candidate bounds tied to the terminal cursor. Suppress premature key forwarding while composing and send committed text exactly once through the safe Ghostty input boundary, retaining terminal-mode-aware key encoding. Use the existing gpui-component text-input facilities for search/palette queries so they can receive native composition, paste, and normal editing. Expected benefit: users can type composed text directly in their shells and search for it in output. Main tradeoff: reconciling text commits with key press/repeat/release events requires care to avoid duplicate bytes or shortcuts escaping composition.

**Focused success check.** With Japanese and Chinese Windows IMEs, compose, edit, cancel, and commit text at a PowerShell prompt and in a text-entry TUI. Verify raw received UTF-8, candidate placement at the cursor, no preedit bytes in the PTY, and exactly one commit. Repeat in search/palette and check ordinary ASCII, AltGr, dead keys, app shortcuts, and Ghostty key-protocol release behavior for regressions.

**Verification record and limits**

The temporary probe is retained at `C:\Users\frixa\AppData\Local\Temp\mightty-review-20260930`, with its own manifest and source. Its `#[path]` modules compile the original repository files; it does not provide a replacement terminal engine. The commands used were:

```powershell
cargo fmt --all -- --check
mise exec -- cargo fmt --all -- --check
mise exec -- cargo test --locked --offline
cargo test --locked --offline

$probe = "$env:TEMP\mightty-review-20260930\Cargo.toml"
cargo test --manifest-path $probe --offline -- --test-threads=1
cargo run --manifest-path $probe --offline
cargo run --manifest-path $probe --offline -- handoff
cargo run --manifest-path $probe --offline -- wsl
```

Cargo was invoked by its absolute executable path where needed. Both formatting checks passed. Both root test attempts stopped at the explicit missing-Ghostty check in `build.rs:31-36`; no root tests ran. The isolated harness passed **16 existing tests**, and the separate worker probes produced the results above. The shell probe used `windows-sys 0.61.2` and `flume 0.12.0`; no GPUI or Ghostty behavior was substituted in those tests.

I examined the requested AGENTS, README, PRD, and roadmap; application startup/activation/quick-terminal services; pane actions and workspace restoration; the binary split model; settings/profile resolution; shell integration; PTY worker and both platform backends; renderer/input/search/graphics paths; safe Ghostty ownership and search wrappers; binding/build guards; font preparation; and Windows packaging, release verification, and upgrade scripts. The checked-out gitlink and `bindings.version` both name Ghostty `4c6215bb8ee186b5c829457a9a9a9c936f2337bf`, but the submodule is uninitialized and all four embedded font files are absent. Consequently full-app checking, Ghostty/ABI tests, GPUI feedback captures, live IME/rendering, Unix execution, mixed-DPI behavior, and signed install/update/handoff verification remain unverified. The missing local prerequisites are a validation limit, not an additional product finding. Only this report was added to the repository.
