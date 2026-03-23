# AGENTS.md

## Project Overview

mightty is a GPU-powered terminal emulator using:
- **GPUI**: Zed's GPU-accelerated UI framework
- **libghostty-vt**: Ghostty's terminal emulator library (built from source via Zig)

## Architecture

```
src/
├── main.rs          # GPUI app entry point
├── lib.rs           # Library exports
└── ghostty/
    ├── mod.rs       # Safe FFI wrapper
    └── types.rs     # Type definitions
```

## Build System

`build.rs` handles:
1. Cloning ghostty to `target/{profile}/build/{pkg}/out/ghostty-src/`
2. Building with `zig build -Demit-lib-vt=true -Dsimd=false`
3. On Windows: copying `ghostty-vt.dll` to target directory

## Key APIs

### Terminal (src/ghostty/mod.rs)

```rust
let mut term = Terminal::new(TerminalOptions {
    rows: 24,
    cols: 80,
    ..Default::default()
})?;

term.write(b"hello")?;
term.resize(30, 100)?;
let (row, col) = term.cursor_pos();
```

### GPUI Elements

See `src/main.rs` for example GPUI app structure. Key components:
- `App::new()` for application context
- `WindowOptions` for window configuration
- `div()` and other GPUI elements for UI

## Development Notes

- libghostty-vt builds as DLL on Windows (static linking has object format issues)
- SIMD must be disabled: `-Dsimd=false`
- DLL auto-copied to `target/debug/` or `target/release/`

## Common Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build
cargo run                # Run debug build
cargo run --example terminal_basic  # Run example
```
