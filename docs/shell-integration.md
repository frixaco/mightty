# Shell integration

mightty uses shell markers for current directories and semantic commands.

PowerShell integration loads automatically when a profile has no explicit
arguments. mightty writes the embedded integration files beside
`settings.json`. Profiles with explicit arguments remain unchanged.

The integration preserves the existing `prompt` function. It also chains the
existing PSReadLine command validation handler. Loading it twice has no effect.

For Bash, source the installed resource after the normal shell startup files:

```bash
source ~/.config/mightty/shell-integration/mightty.bash
```

The Windows resource is below the local application data directory:

```text
%APPDATA%\mightty\shell-integration\mightty.bash
```

The scripts report:

- OSC 7 local working-directory file URIs.
- OSC 133 prompt, input, output, and completion boundaries.

Use the command palette for these actions:

- **Split right in current directory**
- **Split down in current directory**
- **Jump to previous prompt**
- **Jump to next prompt**
- **Select preceding command output**
- **Copy preceding command output**

Semantic actions stay unavailable until the active shell reports prompt
markers. Remote or invalid directory reports remain display data. mightty never
uses them as process working directories.
