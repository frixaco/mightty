# mightty GPUI patches

The application depends on Longbridge GPUI Kit 0.7.1. Its pinned GPUI snapshot is
`gpui-pre` 0.3.8 (Zed revision `279fe070bb389b79652e52065b2f001edcc0b11b`).
`vendor/gpui/` contains that published crate; `vendor/gpui-windows/` contains the
matching `gpui-pre-windows` crate. Both retain their upstream Apache-2.0 license.
Cargo patches both packages so Kit, its components, and mightty share one backend.

Local core changes add focused text commit and immutable prepared/presented scene
metadata, plus the bounded capture interface. Normal event dispatch is public
upstream and no longer needs a patch. Keep application control, protocol and
diagnostics outside these dependencies.

The Windows Direct3D renderer retains two bounded BGRA textures, swaps them only
after successful presentation, and latches readback into an owned staging
texture. Worker readback exports RGBA by channel swizzle and uses D3D11's native
multithread protection. A private hidden window can paint an owned element tree
through the same scene renderer without presentation. Other backends report
unsupported capture. Window hooks associate immutable application metadata with
the prepared scene and the exact successful presentation identity.

The Windows vsync thread invalidates only visible, non-minimized windows. A
minimized window parks its frame callback, so each invalidation fell through to
an iconic `DefWindowProcW` paint 60 times a second; Windows repaints a window
when it is restored or shown.

Cheap metadata getters let `next` wait for a newly prepared scene's successful
presentation. A repeated native present can increment the frame ID without
preparing new application content, so frame ID alone is not a redraw barrier.
Device loss discards retained textures before recreating the renderer.

Limits: 64 MiB per surface, 128 MiB retained textures per window. Capture reports
CPU copy-submission time and readback duration; it does not measure GPU duration.

After upgrading either snapshot, port both patches together and run the control
smoke tests (including UI, snapshot, and presentation modes), `capture_fidelity`,
and the ignored `native_terminal_presentation` GPU test on Windows.
