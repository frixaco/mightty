# mightty GPUI patch

Source: the published `gpui` 0.2.2 crate, under its included Apache-2.0 license.
Cargo patches the dependency here so the application and gpui-component use the
same implementation, including fresh checkouts and release builds.

Local changes expose the normal event dispatch result and a text commit method
using GPUI's existing platform input handler. The explicit `f32` literals in
`taffy.rs` resolve a warning from the current Rust compiler.

Keep application control, protocol and diagnostics outside this dependency.
