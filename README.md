# mightty

A GPU-powered terminal emulator built for productivity.

## Features

- **GPU-accelerated**: Built on GPUI for smooth rendering
- **Terminal tabs**: Toggle sidebar for quick navigation
- **Split panes**: Vertical and horizontal splits for multitasking
- **Zero wasted space**: Clean, minimal interface
- **Performance-first**: Native speed with Rust + Zig backend

## Stack

- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) - GPU-accelerated UI framework
- [libghostty-vt](https://github.com/ghostty-org/ghostty) - Virtual terminal emulator library

## Build

**Prerequisites:**
- [Rust](https://rustup.rs/)
- [Zig](https://ziglang.org/download/) (0.13.0+)

```bash
cargo build --release
```

The build script automatically clones and builds libghostty-vt from source.

## Run

```bash
cargo run
```

## License

MIT
