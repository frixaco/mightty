# ConPTY Shell Bridge

## Purpose
Create and manage pseudo-terminal connection between the UI and actual shell process.

## Interface

```rust
pub struct ConPtyShell {
    pty_handle: HPCON,
    process_handle: HANDLE,
    input_pipe: HANDLE,   // Write to shell
    output_pipe: HANDLE,  // Read from shell
}

impl ConPtyShell {
    /// Spawn a shell with given dimensions
    pub fn spawn(command: &str, rows: u16, cols: u16) -> Result<Self>;
    
    /// Read output from shell (blocking)
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    
    /// Write input to shell
    pub fn write(&mut self, data: &[u8]) -> Result<()>;
    
    /// Resize terminal dimensions
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()>;
    
    /// Shutdown shell and cleanup
    pub fn shutdown(self) -> Result<()>;
}
```

## Implementation

Uses Windows ConPTY API:
- `CreatePseudoConsole()` - create PTY at dimensions
- `CreateProcess()` with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`
- Async read thread feeding libghostty-vt
- Main thread writes keyboard input

## Error Cases
- ConPTY not available (Windows < 10 1809)
- Shell executable not found
- Permission denied
- Process crashed

## Dependencies
- `windows-sys` with `"Win32_System_Console"` feature

## Testing
- Spawn cmd.exe and capture prompt
- Write "echo hello" and verify output
- Resize and verify shell receives SIGWINCH equivalent
