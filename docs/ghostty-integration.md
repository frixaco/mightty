# Ghostty integration

mightty has one terminal-engine upstream: the Ghostty source pinned by the
`ghostty/` git submodule. There are no Ghostty Rust crate dependencies.

## The four similarly named pieces

- `ghostty/` is Ghostty's upstream source code, primarily written in Zig.
- `libghostty-vt` is the native static library that Ghostty's Zig build emits.
  It exposes a public C ABI; it is not a Rust crate in this repository.
- `src/ghostty/ffi.rs` contains generated Rust declarations for that C ABI. It
  has no terminal implementation of its own.
- The rest of `src/ghostty/` is mightty's project-owned safe Rust interface.

Zig `0.16.0` is a build tool here. It compiles the pinned Ghostty source into
the native library. mightty does not call Zig APIs, maintain a separate Zig
wrapper, or require Zig at runtime.

```text
mightty widget code
        |
        v
src/ghostty/ safe Rust interface
        |
        v
src/ghostty/ffi.rs (private generated C bindings)
        |
        v
ghostty/ source submodule --built by Zig--> static libghostty-vt
```

The submodule is pinned rather than fetched during a build. "Update Ghostty"
therefore means advancing that one gitlink to an inspected upstream commit.
Every developer and CI build then uses the same source.

## What this repo owns

- `build.rs` validates and builds the submodule, then links the static library.
- `src/ghostty/terminal.rs` owns the terminal handle and PTY reply callback.
- `src/ghostty/render.rs` owns render state and lending row/cell iterators.
- `src/ghostty/key.rs` owns reusable key events and key encoding.
- `src/ghostty/style.rs` translates only the color/style data mightty renders.
- `src/ghostty/error.rs` translates C result codes.
- `src/ghostty/abi.rs` checks Rust layouts against the target-native library.
- `src/ghostty/ffi.rs` is generated from Ghostty's public headers and is private.
- `src/ghostty/bindings.version` records the exact Ghostty commit and a
  fingerprint of all public headers.

The generated bindings cover Ghostty's complete externally linkable public C
ABI, so adding terminal features never requires changing the generator's
allowlist. Header-only C helpers such as macros and `static inline` functions
are not linkable symbols and are expressed in safe Rust when needed. The safe
Rust layer grows as mightty integrates those features. It does not fetch source
or support dynamic linking.

## Safety invariants

- Every successful C allocation is stored in a non-null owned handle and freed
  exactly once by its Rust owner.
- Ghostty objects are neither `Send` nor `Sync`; they stay on their creating
  thread.
- The PTY callback context is allocated once at terminal creation, so the
  pointer registered with C remains stable even if the Rust `Terminal` moves.
- Callback panics are caught before they can unwind across the C interface.
- A render `Snapshot` mutably borrows its `RenderState`.
- Row and cell iterations are lending iterators. Their returned views cannot
  outlive the snapshot, row, or reusable iterator storage that C references.
- Raw C types and functions never leave the private `ffi` module.
- Tests compare every struct reported by `ghostty_type_json()` with Rust's
  target-native size, alignment, fields, and offsets.

## Build behavior

`build.rs` performs these checks before compiling Rust:

1. `ghostty/build.zig` exists, so an uninitialized submodule fails clearly.
2. the submodule commit equals the commit in `bindings.version`;
3. the fingerprint of `ghostty/include/**/*.h` equals the recorded fingerprint.

It then runs:

```text
zig build
  -Demit-lib-vt=true
  -Demit-xcframework=false
  -Dapp-runtime=none
  -Doptimize=<Cargo-derived mode>
```

Development builds use Zig `Debug`; ordinary release builds use
`ReleaseFast`; size-optimized Cargo profiles use `ReleaseSmall`. Set `ZIG` to
choose a Zig executable.

On Windows the build script copies `ghostty-vt-static.lib` into an isolated
link directory under Cargo's `OUT_DIR` before linking. This prevents Rust's
MSVC linker from accidentally selecting Ghostty's similarly named DLL import
library.

## Updating Ghostty

Normal builds need Rust, Git, and Zig. Binding regeneration additionally needs
`libclang`, because the generator uses `bindgen` only as an explicit developer
tool—not as a build dependency.

```bash
git -C ghostty fetch origin main
git -C ghostty switch --detach origin/main

# If ghostty/build.zig.zon changed its minimum Zig version, update .mise.toml.
mise install

mise exec -- cargo run --manifest-path tools/ghostty-bindings/Cargo.toml

mise exec -- cargo fmt --all -- --check
mise exec -- cargo check
mise exec -- cargo clippy --all-targets -- -D warnings
mise exec -- cargo test
mise exec -- cargo build --release
```

Review the generated diff and the safe wrapper together. If Ghostty changed a
C type or function mightty uses, update the corresponding local wrapper in the
same change. The build intentionally refuses to continue with stale bindings.
