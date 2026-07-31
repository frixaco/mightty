# Workspaces

A workspace saves tabs, pane topology, split ratios, active panes, profiles, and
trusted local working directories. A restore starts new shell processes.
Workspace files do not contain terminal state, process IDs, handles, or GPUI
entities.

Use **Save workspace** in the command palette to save the `default` workspace.
After a workspace exists, the palette provides save and restore commands for
that workspace.

Use a typed key binding to create another named workspace:

```json
{
  "key_bindings": [
    {
      "chord": "ctrl-shift-s",
      "action": {
        "type": "save_workspace",
        "workspace_id": "project-a"
      }
    }
  ]
}
```

Workspace IDs can contain ASCII letters, numbers, `-`, `_`, and `.`.
Workspace files are in the `workspaces` directory beside `settings.json`.

If a saved profile is missing, restore uses the current default profile. If a
saved directory is unavailable, restore uses the profile directory. mightty
shows both conditions in the window.
