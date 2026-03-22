# Mightty - Terminal Emulator

A terminal emulator built with GPUI and libghostty-vt.

## Overview

This project integrates **libghostty-vt** (Ghostty's virtual terminal emulator library) with **GPUI** (Zed's GPU-accelerated UI framework) to create a modern terminal emulator.

## Features

- **libghostty-vt Integration**: Complete terminal emulation with VT sequence parsing
- **GPU-Accelerated UI**: Built on GPUI for smooth, modern interface
- **Cross-Platform**: Designed to work on Windows, macOS, and Linux
- **Safe Rust Bindings**: FFI bindings with safe wrapper API

## Project Structure

```
mightty/
├── build.rs              # Build script that compiles libghostty-vt
├── Cargo.toml           # Rust dependencies
├── src/
│   ├── lib.rs           # Library exports
│   ├── main.rs          # Main application
│   └── ghostty/
│       ├── mod.rs       # Safe Rust FFI bindings
│       └── types.rs     # Type definitions
└── examples/
    └── terminal_basic.rs # Example usage
```

## Prerequisites

- **Rust** (latest stable)
- **Zig** (0.15.x) - Required to build libghostty-vt
- **Git** - For cloning ghostty repository

## Building

### Linux / macOS

```bash
cargo build --release
```

### Windows (Native)

**✅ Working Solution**: The project now builds libghostty-vt as a DLL on Windows native. The `build.rs` script handles this automatically.

#### Automatic Build (Recommended)

Simply run:

```bash
cargo build --release
```

The build script will:
1. Clone the ghostty repository
2. Build libghostty-vt as a DLL using Zig
3. Copy `ghostty-vt.dll` to your target directory
4. Link against the import library

#### Prerequisites for Windows

- **Zig** (0.15.x) - Must be in your PATH
- **Git** - For cloning ghostty
- **Rust** with MSVC toolchain (default)

#### How It Works

On Windows, the build uses a **DLL approach** instead of static linking:
- `ghostty-vt.dll` - The runtime library (copied to `target/debug/` or `target/release/`)
- `ghostty-vt.lib` - The import library for linking

This avoids the object format incompatibility between Zig's static library output and the MSVC linker.

#### Troubleshooting

**"STATUS_DLL_NOT_FOUND" error:**
The DLL wasn't copied to the right location. Try cleaning and rebuilding:
```bash
cargo clean
cargo build
```

**Build fails with Zig errors:**
Make sure you have Zig 0.15.x installed and in your PATH:
```bash
zig version  # Should show 0.15.x
```

## Running Examples

```bash
# Basic terminal example
cargo run --example terminal_basic

# Main application
cargo run
```

## API Usage

```rust
use mightty::ghostty::{Terminal, GhosttyTerminalOptions};

fn main() {
    // Create a new terminal
    let mut terminal = Terminal::new(GhosttyTerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 1000,
    }).expect("Failed to create terminal");
    
    // Write data
    terminal.write_str("Hello, World!\r\n");
    
    // Get cursor position
    let (x, y) = terminal.cursor_pos();
    println!("Cursor at: ({}, {})", x, y);
    
    // Resize
    terminal.resize(100, 50).expect("Failed to resize");
    
    // Get terminal dimensions
    let (cols, rows) = terminal.size();
    println!("Size: {}x{}", cols, rows);
}
```

## Architecture

### libghostty-vt

libghostty-vt provides:
- **Terminal State Management**: Screen buffer, cursor position, scrollback
- **VT Sequence Parsing**: Escape sequences, SGR, OSC, etc.
- **Input Encoding**: Key and mouse event encoding
- **Render State**: Incremental updates for rendering

### FFI Bindings

The `src/ghostty/` module provides:
- Safe Rust wrappers around C API
- Automatic memory management (Drop trait)
- Type-safe enums and structs
- Error handling with Result types

## Troubleshooting

### "zig build failed" on Windows

This is a known issue. Ghostty's build system has complex build-time tools that don't work well with Windows native builds. Use WSL2 or pre-built libraries (see Windows section above).

### "STATUS_DLL_NOT_FOUND" on Windows

The library wasn't linked correctly. Make sure you're using the GNU toolchain:

```bash
rustup target add x86_64-pc-windows-gnu
cargo run --target x86_64-pc-windows-gnu
```

### Missing headers

The build script automatically generates headers from the ghostty source. If you need to access them:

```bash
# After a successful build
find target -name "vt.h" -path "*/ghostty/*"
```

## Development

### Running Tests

```bash
cargo test
```

### Adding Features

The FFI bindings in `src/ghostty/mod.rs` expose the core libghostty-vt API. To add more features:

1. Check the C API in `ghostty/include/ghostty/vt/`
2. Add extern declarations in the `unsafe extern "C"` block
3. Create safe wrapper methods on the `Terminal` struct

## License

MIT License - See Ghostty's license for libghostty-vt terms.

## Resources

- [Ghostty Website](https://ghostty.org)
- [libghostty Documentation](https://libghostty.tip.ghostty.org/)
- [GPUI Documentation](https://docs.rs/gpui/)
- [Ghostling Example](https://github.com/ghostty-org/ghostling) - Minimal C example using libghostty
