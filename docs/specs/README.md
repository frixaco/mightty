# Terminal Implementation Specifications

## Architecture

```
┌─────────────────────────────────────────────────┐
│             Terminal Widget (GPUI)              │
│  - Render cells, cursor, handle input events   │
└──────────────────┬──────────────────────────────┘
                   │ ScreenBuffer, InputMapper
                   ↓
┌─────────────────────────────────────────────────┐
│          Terminal Screen Buffer                 │
│  - Extract cells from libghostty-vt            │
│  - Color mapping, damage tracking              │
└──────────────────┬──────────────────────────────┘
                   │ libghostty-vt
                   ↓
┌─────────────────────────────────────────────────┐
│          Input Event Pipeline                   │
│  - GPUI keys → VT sequences                    │
│  - Application mode handling                   │
└──────────────────┬──────────────────────────────┘
                   │ VT bytes
                   ↓
┌─────────────────────────────────────────────────┐
│          ConPTY Shell Bridge                    │
│  - Windows ConPTY API                          │
│  - Spawn cmd/pwsh/wsl, read/write pipes        │
└─────────────────────────────────────────────────┘
```

## Implementation Order

1. **ConPTY Shell Bridge** - Foundation: get a shell running with I/O
2. **Terminal Screen Buffer** - Read cell data from libghostty-vt
3. **Input Event Pipeline** - Enable keyboard interaction
4. **Terminal Widget** - Put it all together in GPUI

## Specs

| File | Component | Purpose |
|------|-----------|---------|
| [01-conpty-shell-bridge.md](01-conpty-shell-bridge.md) | ConPTY Shell Bridge | Spawn shell processes via Windows ConPTY |
| [02-terminal-screen-buffer.md](02-terminal-screen-buffer.md) | Screen Buffer | Extract renderable cells from libghostty |
| [03-input-event-pipeline.md](03-input-event-pipeline.md) | Input Pipeline | Keyboard events to VT sequences |
| [04-terminal-widget.md](04-terminal-screen-buffer.md) | Terminal Widget | GPUI component integrating all parts |

## Dependencies

- GPUI: UI framework
- libghostty-vt: VT parser
- windows-sys: Windows ConPTY API
