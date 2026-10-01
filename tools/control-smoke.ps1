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
    Write-Output "Control inspection passed: $($instance[0].instance_id); artifacts: $caseDirectory"
} finally {
    if (!$application.HasExited) { Stop-Process -Id $application.Id }
}
