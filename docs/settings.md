# Settings

mightty reads one JSON settings file. It checks the file for changes every two
seconds. A valid change affects new panes. An invalid change keeps the last
valid settings active and shows an error in the window.

Launch-profile discovery is shared across windows and refreshed in the background
every 30 seconds. WSL enumeration has a three-second timeout; a failed refresh
keeps the last successful profile list. Newly installed distributions may take
one refresh interval to appear. File polling never waits for WSL.
The first settings load seeds the discovery cache with the same bounded call,
so an existing WSL default profile can resolve before the first pane starts.

The default file is `%APPDATA%\mightty\settings.json` on Windows. Unix uses
`$XDG_CONFIG_HOME/mightty/settings.json` or `~/.config/mightty/settings.json`.

Set `MIGHTTY_CONFIG_FILE` to use an explicit file. Create `mightty.portable`
beside the application to use `settings.json` in the application directory.

The app and default terminal colors use [Cyberdream's dark palette](https://github.com/scottmckendry/cyberdream.nvim).
Terminal theme overrides still apply; its optional `palette` field accepts 16 ANSI
colors. Colors 16–255 retain the standard xterm color cube and grayscale.

All fields are optional. This example shows the main settings:

```json
{
  "app": {
    "sidebar_visible": true,
    "bell_notifications": true,
    "quick_terminal": {
      "enabled": false,
      "hotkey": "ctrl-`",
      "width_ratio": 0.8,
      "height_ratio": 0.45,
      "hide_on_focus_loss": true
    }
  },
  "terminal": {
    "initial_rows": 30,
    "initial_cols": 100,
    "scrollback": 10000,
    "cursor_style": "line",
    "cursor_blink": true,
    "blink_interval_ms": 500,
    "terminal_clipboard_policy": "deny",
    "font_family": "JetBrainsMono Nerd Font Mono",
    "font_size_px": 16,
    "theme": {
      "foreground": "#ffffff",
      "background": "#16181a",
      "cursor": "#ffffff",
      "selection": "#3c4048"
    }
  },
  "profiles": [
    {
      "id": "work",
      "label": "Work PowerShell",
      "executable": "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
      "arguments": ["-NoLogo"],
      "starting_directory": "C:\\work",
      "environment": {
        "TERM": "xterm-256color"
      }
    }
  ],
  "default_profile": "work",
  "key_bindings": [
    {
      "chord": "ctrl-shift-p",
      "action": {"type": "command_palette"}
    }
  ]
}
```

Profile IDs are stable. Workspaces and actions use the ID instead of the label.
An explicit profile can override a discovered profile with the same ID.

mightty discovers PowerShell, Command Prompt, WSL distributions, and Git Bash
on Windows. Unix uses `$SHELL` and falls back to `/bin/sh`.

`bell_notifications` flashes the Windows taskbar button when an unfocused
window receives a terminal bell. Active windows do not request attention.

AppInstaller controls updates for installed Windows packages. It checks on
launch and in the background. See
[Windows distribution](windows-distribution.md) for package details.
