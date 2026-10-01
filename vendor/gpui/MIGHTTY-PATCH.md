# mightty GPUI patch

Source: the published `gpui` 0.2.2 crate, under its included Apache-2.0 license.
Cargo patches the dependency here so the application and gpui-component use the
same implementation, including fresh checkouts and release builds.

Local changes expose the normal event dispatch result and a text commit method
using GPUI's existing platform input handler. The explicit `f32` literals in
`taffy.rs` resolve a warning from the current Rust compiler.

Keep application control, protocol and diagnostics outside this dependency.

The Windows Direct3D renderer retains two bounded BGRA textures, swaps them only
after successful presentation, and latches readback into an owned staging
texture. Worker readback exports RGBA by channel swizzle and uses D3D11's native
multithread protection. A private hidden window can paint an owned element tree
through the same scene renderer without presentation. Other backends report
unsupported capture. Window hooks associate immutable application metadata with
the prepared scene and the exact successful presentation identity.
Cheap metadata getters let `next` wait for a newly prepared scene's successful
presentation. A repeated native present can increment the frame ID without
preparing new application content, so frame ID alone is not a redraw barrier.

Limits: 64 MiB per surface, 128 MiB retained textures per window. Capture reports
CPU copy-submission time and readback duration; it does not measure GPU duration.
