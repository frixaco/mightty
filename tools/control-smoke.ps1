param([string]$Executable = "$PSScriptRoot/../target/debug/mightty.exe", [switch]$UiOnly, [switch]$SnapshotOnly)
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$caseDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('mightty-control-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $caseDirectory | Out-Null
@{
    default_profile = 'fixture'
    profiles = @(@{ id = 'fixture'; label = 'Control fixture'; executable = 'pwsh.exe'; arguments = @('-NoLogo', '-NoProfile') })
    terminal = @{ cursor_blink = $false }
    app = @{ quick_terminal = @{ enabled = $true; hide_on_focus_loss = $false } }
    key_bindings = @(@{ chord = 'ctrl-alt-q'; action = @{ type = 'toggle_quick_terminal' } })
} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'settings.json') -Encoding utf8NoBOM
$application = Start-Process -FilePath $Executable -ArgumentList @('--test-instance', '--data-dir', ('"' + $caseDirectory + '"')) -WorkingDirectory $caseDirectory -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $caseDirectory 'stdout.log') -RedirectStandardError (Join-Path $caseDirectory 'stderr.log')
try {
    $deadline = [datetime]::UtcNow.AddSeconds(20)
    do {
        $instances = (& $Executable ctl instances --json | ConvertFrom-Json).result
        $instance = @($instances | Where-Object pid -eq $application.Id)
        if ($instance.Count -eq 1) { break }
        if ($application.HasExited) { throw 'Isolated instance exited during startup' }
        Start-Sleep -Milliseconds 100
    } while ([datetime]::UtcNow -lt $deadline)
    if ($instance.Count -ne 1) { throw 'Isolated instance was not discovered' }
    function Invoke-Control([string[]]$Command) {
        $separator = [Array]::IndexOf($Command, '--')
        $selectors = @('--instance', $instance[0].instance_id, '--json')
        $controlArguments = if ($separator -ge 1) { $Command[0..($separator - 1)] + $selectors + $Command[$separator..($Command.Length - 1)] } else { $Command + $selectors }
        $response = & $Executable ctl @controlArguments | ConvertFrom-Json
        if (!$response.ok) { throw ("Command: $($Command -join ' '); " + ($response.error | ConvertTo-Json -Depth 8) + "; logs: $caseDirectory") }
        $response.result
    }
    $state = Invoke-Control @('state')
    if ($state.windows.Count -ne 1) { throw 'Isolated startup opened unexpected windows' }
    $pane = $state.windows[0].tabs[0].panes[0].pane_id
    if ($UiOnly) {
        $null = Invoke-Control @('window', 'focus', '--window', 'w1')
        $shown = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
        $click = @(@{ type = 'pointer'; event = 'press'; button = 'left'; x = 20; y = 17 }, @{ type = 'pointer'; event = 'release'; button = 'left'; x = 20; y = 17 })
        $click | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'sidebar-click.json') -Encoding utf8NoBOM
        $null = Invoke-Control @('ui', 'input', '--window', 'w1', '--file', (Join-Path $caseDirectory 'sidebar-click.json'))
        if ((Invoke-Control @('state', '--window', 'w1')).sidebar_visible) { throw 'Sidebar button did not hide the sidebar' }
        $hidden = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
        $null = Invoke-Control @('ui', 'input', '--window', 'w1', '--file', (Join-Path $caseDirectory 'sidebar-click.json'))
        if (!(Invoke-Control @('state', '--window', 'w1')).sidebar_visible) { throw 'Sidebar button did not restore the sidebar' }
        $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'b', '--mod', 'ctrl')
        if ((Invoke-Control @('state', '--window', 'w1')).sidebar_visible) { throw 'Sidebar shortcut did not hide the sidebar' }
        $null = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
        $null = Invoke-Control @('ui', 'input', '--window', 'w1', '--file', (Join-Path $caseDirectory 'sidebar-click.json'))
        if (!(Invoke-Control @('state', '--window', 'w1')).sidebar_visible) { throw 'Sidebar button did not follow the keyboard toggle' }
        Write-Output "Sidebar shown: $($shown.image); hidden: $($hidden.image)"
        $projectDirectory = Join-Path $caseDirectory 'mightty'
        $otherDirectory = Join-Path $projectDirectory 'docs'
        New-Item -ItemType Directory -Path $otherDirectory | Out-Null
        $named = Invoke-Control @('tab', 'new', '--window', 'w1', '--cwd', $projectDirectory, '--exec', 'pwsh.exe', '--', '-NoLogo', '-NoProfile')
        $null = Invoke-Control @('wait', '--tab', $named.tab_id, '--condition', 'title-equals', '--value', 'mightty · pwsh', '--timeout', '10s')
        $directoryUri = ([uri]($otherDirectory + [IO.Path]::DirectorySeparatorChar)).AbsoluteUri
        $command = "Set-Location '$($otherDirectory.Replace("'", "''"))'; [Console]::Write([char]27 + ']7;$directoryUri' + [char]7)"
        @(@{ type = 'text'; text = $command }, @{ type = 'key'; key = 'enter' }) | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'title-input.json') -Encoding utf8NoBOM
        $null = Invoke-Control @('pane', 'input', '--pane', $named.panes[0].pane_id, '--file', (Join-Path $caseDirectory 'title-input.json'))
        $null = Invoke-Control @('wait', '--tab', $named.tab_id, '--condition', 'title-equals', '--value', 'docs · pwsh', '--timeout', '10s')
        $null = Invoke-Control @('tab', 'select', '--tab', $named.tab_id)
        $compact = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
        $labels = (Get-Content -LiteralPath (Join-Path $compact.directory 'frame.json') -Raw -Encoding utf8 | ConvertFrom-Json).labels
        if (@($labels | Where-Object { $_.badge -ne '' -or $_.geometry.line_height_px -ne 28 }).Count) { throw 'Sidebar retained tab numbers or oversized rows' }
        $namedLabel = @($labels | Where-Object tab_id -eq $named.tab_id)[0]
        if ($namedLabel.chosen_title -ne 'docs · pwsh' -or $namedLabel.provenance.source -ne 'selected_pane') { throw 'Friendly title or provenance is incorrect' }
        Write-Output "Compact sidebar: $($compact.image)"
        @(@{ type = 'text'; text = "[Console]::Write([char]27 + ']0;Build logs' + [char]7)" }, @{ type = 'key'; key = 'enter' }) | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'title-input.json') -Encoding utf8NoBOM
        $null = Invoke-Control @('pane', 'input', '--pane', $named.panes[0].pane_id, '--file', (Join-Path $caseDirectory 'title-input.json'))
        $null = Invoke-Control @('wait', '--tab', $named.tab_id, '--condition', 'title-equals', '--value', 'Build logs', '--timeout', '10s')
        $null = Invoke-Control @('tab', 'close', '--tab', $named.tab_id)
    }
    if ($SnapshotOnly) {
        $null = Invoke-Control @('window', 'focus', '--window', 'w1')
        $null = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
        $presented = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'presented', '--out', $caseDirectory)
        $next = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
        $first = Get-Content -LiteralPath $presented.manifest -Raw | ConvertFrom-Json
        $second = Get-Content -LiteralPath $next.manifest -Raw | ConvertFrom-Json
        if ([long]$second.frame_id -le [long]$first.frame_id -or [long]$second.frame_revision -le [long]$first.frame_revision) { throw 'Next frame did not advance the presented scene' }
        if (!(Test-Path -LiteralPath $presented.image) -or !(Test-Path -LiteralPath $next.image)) { throw 'Renderer capture image missing' }
        $fixtureTitle = 'Inactive title wider than the sidebar: wrapping and clipping must stay visible in the window capture'
        $marker = 'raster-' + [guid]::NewGuid().ToString('N')
        $fixture = "[Console]::OutputEncoding = [Text.UTF8Encoding]::new(`$false); `$PSStyle.OutputRendering = 'Ansi'; Clear-Host; [Console]::Write([char]27 + ']0;$fixtureTitle' + [char]7); [Console]::WriteLine([char]27 + '[31mflower ' + [char]0x2740 + ' nerd ' + [char]0xf17c + ' wide ' + [char]0x754c + ' combining e' + [char]0x301 + ' => !=' + [char]27 + '[0m'); [Console]::WriteLine([char]27 + '[48;2;12;34;56m     ' + [char]27 + '[0m'); Write-Host ('wrapped-' + ('w' * 160)); Write-Host '$marker'"
        $background = Invoke-Control @('tab', 'new', '--window', 'w1', '--cwd', $caseDirectory, '--exec', 'pwsh.exe', '--', '-NoLogo', '-NoProfile', '-NoExit', '-Command', $fixture)
        $backgroundPane = $background.panes[0].pane_id
        $null = Invoke-Control @('wait', '--pane', $backgroundPane, '--text', $marker, '--timeout', '10s')
        $before = Invoke-Control @('state')
        $offscreen = Invoke-Control @('snapshot', '--tab', $background.tab_id, '--layout', $background.layout_token, '--out', $caseDirectory)
        $paneCapture = Invoke-Control @('snapshot', '--pane', $backgroundPane, '--out', $caseDirectory)
        $source = (Get-Content -LiteralPath (Join-Path $paneCapture.directory 'frame.json') -Raw | ConvertFrom-Json).panes[0].source
        if (!(($source.rows.text -join "`n").Contains([char]0x2740)) -or !($source.rows | Where-Object wrapped)) { throw 'Capture lost Unicode or soft wrapping' }
        $cells = Invoke-Control @('pane', 'read', '--pane', $backgroundPane, '--viewport', '--format', 'cells')
        if (!($cells.rows.cells | Where-Object { $_.bg.hex -eq 0x0c2238 })) { throw 'Diagnostic read lost blank background color' }
        $after = Invoke-Control @('state')
        if ($before.windows[0].active_tab_id -ne $after.windows[0].active_tab_id -or $before.windows[0].tabs[1].panes[0].pty_size.cols -ne $after.windows[0].tabs[1].panes[0].pty_size.cols) { throw 'Offscreen capture changed selection or PTY geometry' }
        if (!(Test-Path -LiteralPath $offscreen.image) -or !(Test-Path -LiteralPath $paneCapture.image)) { throw 'Background image missing' }
        $titles = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
        $labels = (Get-Content -LiteralPath (Join-Path $titles.directory 'frame.json') -Raw | ConvertFrom-Json).labels
        $label = @($labels | Where-Object tab_id -eq $background.tab_id)[0]
        if ($label.chosen_title -ne $fixtureTitle -or $label.geometry.overflow -ne 'ellipsis' -or $label.geometry.bounds.y -lt 20) { throw 'Inactive title geometry missing or incorrect' }
        $crop = Invoke-Control @('snapshot', '--pane', $pane, '--frame', 'presented', '--out', $caseDirectory)
        $absent = & $Executable ctl snapshot --tab $background.tab_id --frame presented --out $caseDirectory --instance $instance[0].instance_id --json | ConvertFrom-Json
        if ($absent.ok -or $absent.error.code -ne 'target_not_in_frame') { throw 'Presented capture accepted an invisible tab' }
        $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'f12', '--mod', 'ctrl', '--mod', 'shift')
        $feedbackRoot = Join-Path $caseDirectory 'captures'
        $deadline = [datetime]::UtcNow.AddSeconds(10)
        do {
            $feedback = @(Get-ChildItem -LiteralPath $feedbackRoot -Directory -Filter 'capture-*' -ErrorAction SilentlyContinue | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'manifest.json') })
            if ($feedback.Count) { break }
            Start-Sleep -Milliseconds 100
        } while ([datetime]::UtcNow -lt $deadline)
        if (!$feedback.Count) { throw 'Feedback shortcut did not publish a renderer bundle' }
        $feedbackManifest = Get-Content -LiteralPath (Join-Path $feedback[0].FullName 'manifest.json') -Raw | ConvertFrom-Json
        if ($feedbackManifest.mode -ne 'presented' -or $feedbackManifest.status -ne 'complete') { throw 'Feedback shortcut bypassed the presented capture coordinator' }
        Write-Output "Inactive titles: $($titles.image); visible crop: $($crop.image)"
        Write-Output "Offscreen tab: $($offscreen.image); pane: $($paneCapture.image)"
        Write-Output "Renderer readback passed; presented: $($presented.image); next: $($next.image)"; return
    }
    $null = Invoke-Control @('ui', 'sidebar', '--window', 'w1', '--visible', 'false')
    $sidebar = Invoke-Control @('state', '--window', 'w1')
    if ($sidebar.sidebar_visible) { throw 'Sidebar desired state failed' }
    $null = Invoke-Control @('ui', 'sidebar', '--window', 'w1', '--visible', 'true')
    $uiSteps = @(@{ type = 'key'; key = 'p'; modifiers = @('ctrl', 'shift') }, @{ type = 'text'; text = 'split' })
    $uiSteps | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'ui-input.json') -Encoding utf8NoBOM
    $uiResult = Invoke-Control @('ui', 'input', '--window', 'w1', '--file', (Join-Path $caseDirectory 'ui-input.json'))
    $uiResult | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'ui-result.json') -Encoding utf8NoBOM
    $palette = Invoke-Control @('state', '--window', 'w1')
    if (!$palette.palette_open -or $palette.palette.query -ne 'split') { $palette | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $caseDirectory 'ui-state.json') -Encoding utf8NoBOM; throw "Normal UI shortcut/text routing failed; artifacts: $caseDirectory" }
    $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'escape')
    $null = Invoke-Control @('ui', 'search', '--pane', $pane, '--open', 'true', '--query', 'fixture')
    $search = Invoke-Control @('state', '--pane', $pane)
    if ($search.search.query -ne 'fixture') { throw 'Search desired state failed' }
    $null = Invoke-Control @('ui', 'search', '--pane', $pane, '--open', 'false')
    $split = Invoke-Control @('pane', 'split', '--pane', $pane, '--direction', 'right')
    $left = @($split.panes | Where-Object pane_id -eq $pane)[0].computed_bounds
    $x = $left.x + $left.width + 2
    $y = $left.y + $left.height / 2
    $drag = @(@{ type = 'pointer'; event = 'move'; x = $x; y = $y }, @{ type = 'pointer'; event = 'press'; button = 'left'; x = $x; y = $y }, @{ type = 'pointer'; event = 'move'; x = $x + 8; y = $y }, @{ type = 'pointer'; event = 'move'; x = $x + 40; y = $y }, @{ type = 'pointer'; event = 'release'; button = 'left'; x = $x + 40; y = $y })
    $drag | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'drag.json') -Encoding utf8NoBOM
    $windowState = Invoke-Control @('state', '--window', 'w1')
    $null = Invoke-Control @('ui', 'input', '--window', 'w1', '--if-layout', $windowState.layout_token, '--file', (Join-Path $caseDirectory 'drag.json'))
    $dragged = Invoke-Control @('state', '--tab', $split.tab_id)
    if ($dragged.layout_token -eq $split.layout_token) { throw 'Real divider drag did not change layout' }
    $null = Invoke-Control @('pane', 'close', '--pane', $split.new_pane_id)
    if ($UiOnly) { Write-Output "UI control passed; artifacts: $caseDirectory"; return }
    $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'q', '--mod', 'ctrl', '--mod', 'alt')
    $quick = @((Invoke-Control @('state')).windows | Where-Object window_id -ne 'w1')[0]
    if (!$quick.window_id) { throw 'Quick terminal did not open through the UI action' }
    $null = Invoke-Control @('tab', 'close', '--tab', $quick.active_tab_id)
    $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'q', '--mod', 'ctrl', '--mod', 'alt')
    $replacement = @((Invoke-Control @('state')).windows | Where-Object window_id -ne 'w1')[0]
    if (!$replacement.window_id -or $replacement.window_id -eq $quick.window_id) { throw 'Quick terminal reused a removed window ID' }
    $staleWindow = & $Executable ctl state --window $quick.window_id --instance $instance[0].instance_id --json | ConvertFrom-Json
    if ($staleWindow.ok) { throw 'Stale window ID resolved to the replacement' }
    $null = Invoke-Control @('tab', 'close', '--tab', $replacement.active_tab_id)
    $capabilities = Invoke-Control @('capabilities')
    if (!$capabilities.'$defs'.terminal_step -or !$capabilities.operations[0].argument_schema) { throw 'Published protocol schema missing' }
    $null = Invoke-Control @('profiles')
    $settings = Get-Content -LiteralPath (Join-Path $caseDirectory 'settings.json') -Raw | ConvertFrom-Json
    $settings.terminal | Add-Member -NotePropertyName font_size_px -NotePropertyValue 18 -Force
    $settings | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'settings.json') -Encoding utf8NoBOM
    $null = Invoke-Control @('wait', '--window', 'w1', '--condition', 'settings-generation', '--at-least', '2', '--timeout', '10s')
    $provenance = Invoke-Control @('state', '--pane', $pane)
    if ($provenance.launch.settings_generation -ne '1' -or $provenance.effective_settings.font_size_px -ne 16 -or $provenance.effective_settings.bindings_generation -ne '2' -or !$provenance.title.observed_at -or !$provenance.terminal_status.value.cursor_visible) { throw 'State lost effective settings, title provenance, or terminal status' }
    $text = Invoke-Control @('pane', 'read', '--pane', $pane, '--tail', '100')
    if ($text.source -ne 'active_buffer_tail') { throw 'Read source mislabeled' }
    $bad = & $Executable ctl pane read --pane p999999 --instance $instance[0].instance_id --json | ConvertFrom-Json
    if ($bad.ok -or $bad.error.effect -ne 'none') { throw 'Stale target was accepted' }
    $originalTab = $state.windows[0].active_tab_id
    $windowResize = Invoke-Control @('window', 'resize', '--window', 'w1', '--width', '1000', '--height', '700')
    if ($windowResize.bounds.width -ne 1000 -or $windowResize.bounds.height -ne 700) { throw 'Window resize did not report client dimensions' }
    $null = Invoke-Control @('wait', '--window', 'w1', '--condition', 'layout-ready', '--layout', $windowResize.layout_token)
    $windowRead = Invoke-Control @('pane', 'read', '--window', 'w1', '--tail', '100')
    if ($windowRead.pane_id -ne $pane) { throw 'Window read did not resolve the selected pane' }
    $background = Invoke-Control @('tab', 'new', '--window', 'w1', '--profile', 'fixture', '--cwd', $caseDirectory, '--env', 'FIXTURE_VALUE=hello world')
    $backgroundPane = $background.panes[0].pane_id
    if ($background.panes[0].computed_bounds.width -le 0 -or !$background.completion.pty_acknowledged) { throw 'Background layout was not acknowledged' }
    $after = Invoke-Control @('state')
    if ($after.windows[0].active_tab_id -ne $originalTab) { throw 'Background creation changed selection' }
    $created = @()
    foreach ($direction in @('left', 'right', 'up', 'down')) {
        $split = Invoke-Control @('pane', 'split', '--pane', $backgroundPane, '--direction', $direction, '--ratio', '0.35', '--profile', 'fixture')
        $created += $split.new_pane_id
        if ($split.selected_pane_id -ne $backgroundPane) { throw 'Background split changed selected pane' }
    }
    $tabState = Invoke-Control @('state', '--tab', $background.tab_id)
    $oldToken = $tabState.layout_token
    $resize = Invoke-Control @('pane', 'resize', '--pane', $backgroundPane, '--edge', 'right', '--delta-px', '35', '--if-layout', $oldToken)
    if (!$resize.completion.pty_acknowledged) { throw 'Resize was not acknowledged' }
    $stale = & $Executable ctl pane resize --pane $backgroundPane --edge right --delta-px 10 --if-layout $oldToken --instance $instance[0].instance_id --json | ConvertFrom-Json
    if ($stale.ok -or $stale.error.code -ne 'precondition_failed') { throw 'Stale layout mutation accepted' }
    $null = Invoke-Control @('pane', 'zoom', '--pane', $backgroundPane, '--enabled', 'true')
    $marker = 'done-' + [guid]::NewGuid().ToString('N')
    $suffix = $marker.Substring(5)
    $steps = @(@{ type = 'text'; text = "Write-Output ('done-' + '$suffix'); Write-Output `$env:MIGHTTY_PANE_ID; Write-Output `$env:FIXTURE_VALUE" }, @{ type = 'key'; key = 'enter' })
    $steps | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'input.json') -Encoding utf8NoBOM
    $input = Invoke-Control @('pane', 'input', '--pane', $backgroundPane, '--file', (Join-Path $caseDirectory 'input.json'))
    if ($input.completion.completed_steps -ne 2) { throw 'Ordered input completion is wrong' }
    $null = Invoke-Control @('wait', '--pane', $backgroundPane, '--text', $marker, '--after-output', $input.output_cursor, '--timeout', '10s')
    $read = Invoke-Control @('pane', 'read', '--pane', $backgroundPane, '--tail', '100')
    $read | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $caseDirectory 'read.json') -Encoding utf8NoBOM
    if (!$read.text.Contains($marker) -or !$read.text.Contains('hello world')) { throw "Ordered input or launch environment failed; read: $caseDirectory/read.json" }
    foreach ($createdPane in $created) { $closed = Invoke-Control @('pane', 'close', '--pane', $createdPane) }
    $remaining = Invoke-Control @('state', '--tab', $background.tab_id)
    if ($remaining.selected_pane_id -ne $backgroundPane) { throw 'Closing unselected panes changed selection' }
    $null = Invoke-Control @('tab', 'close', '--tab', $background.tab_id)
    $outcome = Invoke-Control @('state', '--pane', $backgroundPane)
    if (!$outcome.removed_at -or !$outcome.final_tail.Contains($marker) -or $outcome.removal_reason -ne 'tab_closed' -or !$outcome.output_cursor.Contains(":${backgroundPane}:")) { throw 'Removed pane outcome unavailable or uncorrelated' }
    $null = Invoke-Control @('wait', '--pane', $backgroundPane, '--condition', 'process-exited', '--timeout', '10s')
    $events = @(& $Executable ctl events --instance $instance[0].instance_id --json --timeout 2s | ForEach-Object { $_ | ConvertFrom-Json })
    if ($events[0].type -ne 'state' -or $events[-1].type -ne 'end' -or $events[-1].protocol_version -ne 1) { throw 'Event subscription did not deliver initial state and clean end' }
    $saved = & $Executable ctl state --saved --data-dir $caseDirectory --instance $instance[0].instance_id --json | ConvertFrom-Json
    if (!$saved.ok -or $saved.result.source -ne 'saved') { throw 'Saved state unavailable' }
    $null = Invoke-Control @('tab', 'close', '--tab', $originalTab)
    if (!$application.WaitForExit(10000)) { throw 'Final tab close did not exit the instance' }
    $saved = & $Executable ctl state --saved --data-dir $caseDirectory --instance $instance[0].instance_id --json | ConvertFrom-Json
    if (!$saved.ok -or !$saved.result.orderly_shutdown -or $saved.result.windows.Count -ne 0) { throw 'Final state did not record orderly shutdown and closed windows' }
    Write-Output "Control inspection passed: $($instance[0].instance_id); artifacts: $caseDirectory"
} finally {
    if (!$application.HasExited) { Stop-Process -Id $application.Id }
}
