param([string]$Executable = "$PSScriptRoot/../target/debug/mightty.exe")
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$caseDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('mightty-control-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $caseDirectory | Out-Null
@{
    default_profile = 'fixture'
    profiles = @(@{ id = 'fixture'; label = 'Control fixture'; executable = 'pwsh.exe'; arguments = @('-NoLogo', '-NoProfile') })
    terminal = @{ cursor_blink = $false }
} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'settings.json') -Encoding utf8NoBOM
$application = Start-Process -FilePath $Executable -ArgumentList @('--test-instance', '--data-dir', ('"' + $caseDirectory + '"')) -PassThru -WindowStyle Hidden
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
        if (!$response.ok) { throw ($response.error | ConvertTo-Json -Depth 8) }
        $response.result
    }
    $state = Invoke-Control @('state')
    if ($state.windows.Count -ne 1) { throw 'Isolated startup opened unexpected windows' }
    $pane = $state.windows[0].tabs[0].panes[0].pane_id
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
    $deadline = [datetime]::UtcNow.AddSeconds(10)
    do {
        $read = Invoke-Control @('pane', 'read', '--pane', $backgroundPane, '--tail', '100')
        if ($read.text.Contains($marker)) { break }
        Start-Sleep -Milliseconds 100
    } while ([datetime]::UtcNow -lt $deadline)
    $read | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $caseDirectory 'read.json') -Encoding utf8NoBOM
    if (!$read.text.Contains($marker) -or !$read.text.Contains('hello world')) { throw "Ordered input or launch environment failed; read: $caseDirectory/read.json" }
    foreach ($createdPane in $created) { $closed = Invoke-Control @('pane', 'close', '--pane', $createdPane) }
    $remaining = Invoke-Control @('state', '--tab', $background.tab_id)
    if ($remaining.selected_pane_id -ne $backgroundPane) { throw 'Closing unselected panes changed selection' }
    $null = Invoke-Control @('tab', 'close', '--tab', $background.tab_id)
    Write-Output "Control inspection passed: $($instance[0].instance_id); artifacts: $caseDirectory"
} finally {
    if (!$application.HasExited) { Stop-Process -Id $application.Id }
}
