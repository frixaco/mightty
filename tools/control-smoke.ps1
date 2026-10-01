param([string]$Executable = "$PSScriptRoot/../target/debug/mightty.exe", [switch]$UiOnly)
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$caseDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('mightty-control-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $caseDirectory | Out-Null
@{
    default_profile = 'fixture'
    profiles = @(@{ id = 'fixture'; label = 'Control fixture'; executable = 'pwsh.exe'; arguments = @('-NoLogo', '-NoProfile') })
    terminal = @{ cursor_blink = $false }
} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'settings.json') -Encoding utf8NoBOM
$application = Start-Process -FilePath $Executable -ArgumentList @('--test-instance', '--data-dir', ('"' + $caseDirectory + '"')) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $caseDirectory 'stdout.log') -RedirectStandardError (Join-Path $caseDirectory 'stderr.log')
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
        $response = & $Executable ctl @Command --instance $instance[0].instance_id --json | ConvertFrom-Json
        if (!$response.ok) { throw ("Command: $($Command -join ' '); " + ($response.error | ConvertTo-Json -Depth 8) + "; logs: $caseDirectory") }
        $response.result
    }
    $state = Invoke-Control @('state')
    if ($state.windows.Count -ne 1) { throw 'Isolated startup opened unexpected windows' }
    $pane = $state.windows[0].tabs[0].panes[0].pane_id
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
    $null = Invoke-Control @('capabilities')
    $null = Invoke-Control @('profiles')
    $text = Invoke-Control @('pane', 'read', '--pane', $pane, '--tail', '100')
    if ($text.source -ne 'active_buffer_tail') { throw 'Read source mislabeled' }
    $bad = & $Executable ctl pane read --pane p999999 --instance $instance[0].instance_id --json | ConvertFrom-Json
    if ($bad.ok -or $bad.error.effect -ne 'none') { throw 'Stale target was accepted' }
    $originalTab = $state.windows[0].active_tab_id
    $windowResize = Invoke-Control @('window', 'resize', '--window', 'w1', '--width', '1000', '--height', '700')
    if ($windowResize.bounds.width -ne 1000 -or $windowResize.bounds.height -ne 700) { throw 'Window resize did not report client dimensions' }
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
    if (!$outcome.removed_at -or !$outcome.final_tail.Contains($marker)) { throw 'Removed pane outcome unavailable' }
    $null = Invoke-Control @('wait', '--pane', $backgroundPane, '--condition', 'process-exited', '--timeout', '10s')
    $events = @(& $Executable ctl events --instance $instance[0].instance_id --json --timeout 2s | ForEach-Object { $_ | ConvertFrom-Json })
    if ($events[0].type -ne 'state' -or $events[-1].type -ne 'end') { throw 'Event subscription did not deliver initial state and clean end' }
    $saved = & $Executable ctl state --saved --data-dir $caseDirectory --instance $instance[0].instance_id --json | ConvertFrom-Json
    if (!$saved.ok -or $saved.result.source -ne 'saved') { throw 'Saved state unavailable' }
    Write-Output "Control inspection passed: $($instance[0].instance_id); artifacts: $caseDirectory"
} finally {
    if (!$application.HasExited) { Stop-Process -Id $application.Id }
}
