param(
    [string]$Executable = "$PSScriptRoot/../target/debug/examples/pty_experiment.exe",
    [string]$GuiExecutable = "$PSScriptRoot/../target/debug/mightty.exe",
    [string]$OutputDirectory = "$PSScriptRoot/../artifacts/pty-experiment/$([datetime]::Now.ToString('yyyyMMdd-HHmmss'))",
    [string[]]$Cases = @('cmd', 'pwsh', 'powershell', 'launch', 'env-clear', 'cwd-default', 'invalid', 'unicode-vt', 'mode-probe', 'protocol-probe', 'resize', 'resize-storm', 'input', 'encoded-keys', 'unicode-input', 'bulk-input', 'bulk-output', 'idle-close', 'blocked-input-close', 'blocked-output-close', 'repeat', 'concurrent', 'child-tree-close', 'killer-contract'),
    [int]$Repetitions = 3,
    [int]$TimeoutSeconds = 45,
    [string[]]$Backends = @('native', 'portable', 'native-flags', 'portable-system'),
    [string[]]$GuiModes = @('Control', 'ControlExplicitTitle', 'UiOnly', 'SnapshotOnly', 'PresentationOnly', 'OutputStress'),
    [switch]$SkipGui
)
$ErrorActionPreference = 'Stop'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$results = [Collections.Generic.List[object]]::new()
function Run-Case([string]$Backend, [string]$Case, [int]$Round, [string]$Program, [string[]]$Arguments, [int]$Timeout) {
    $start = [Diagnostics.ProcessStartInfo]::new($Program)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.WorkingDirectory = (Resolve-Path -LiteralPath "$PSScriptRoot/..").Path
    $start.Environment['MIGHTTY_PTY_BACKEND'] = $Backend
    if ($Backend -eq 'portable-system') {
        # The crate loads conpty.dll by name before falling back to kernel32.
        # Remove only its PATH search candidates in this disposable process.
        $path = @($start.Environment['PATH'] -split ';' | Where-Object { $_ -and !(Test-Path -LiteralPath (Join-Path $_ 'conpty.dll')) })
        $start.Environment['PATH'] = $path -join ';'
    }
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $completed = $process.WaitForExit($Timeout * 1000)
    if (!$completed) { $process.Kill($true); $process.WaitForExit(5000) | Out-Null }
    $output = $stdout.GetAwaiter().GetResult()
    $errors = $stderr.GetAwaiter().GetResult()
    $exitCode = $process.ExitCode
    $process.Dispose()
    $prefix = Join-Path $OutputDirectory "$Backend-$Case-$Round"
    [IO.File]::WriteAllText("$prefix.stdout.log", $output)
    [IO.File]::WriteAllText("$prefix.stderr.log", $errors)
    $measurement = $null
    if ($completed -and $exitCode -eq 0 -and !$Case.StartsWith('gui-')) {
        try { $measurement = $output.Trim() | ConvertFrom-Json } catch { $errors += "`nInvalid result JSON: $_" }
    }
    $ok = $completed -and $exitCode -eq 0 -and ($Case.StartsWith('gui-') -or $measurement.ok)
    $result = [ordered]@{ backend = $Backend; case = $Case; round = $Round; ok = $ok; timeout = !$completed; exit_code = $exitCode; wall_ms = $clock.Elapsed.TotalMilliseconds; measurement = $measurement; stdout = "$prefix.stdout.log"; stderr = "$prefix.stderr.log" }
    $results.Add($result)
    $results | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'results.json') -Encoding utf8NoBOM
    Write-Host "$Backend $Case [$Round]: $(if ($ok) {'PASS'} elseif (!$completed) {'TIMEOUT'} else {'FAIL'}) ($([math]::Round($result.wall_ms)) ms)"
}
for ($round = 1; $round -le $Repetitions; $round++) {
    $orderedBackends = @($Backends)
    if (!($round % 2)) { [array]::Reverse($orderedBackends) }
    foreach ($case in $Cases) {
        # One upstream-specific API contract check, not a native comparison.
        foreach ($backend in $orderedBackends) {
            if ($case -eq 'killer-contract' -and !$backend.StartsWith('portable')) { continue }
            Run-Case $backend $case $round $Executable @($case) $TimeoutSeconds
        }
    }
}
if (!$SkipGui) {
    $GuiExecutable = (Resolve-Path -LiteralPath $GuiExecutable).Path
    $pwsh = (Get-Command pwsh.exe).Source
    foreach ($mode in $GuiModes) {
        foreach ($backend in $Backends) {
            $arguments = @('-NoProfile', '-File', "$PSScriptRoot/control-smoke.ps1", '-Executable', $GuiExecutable)
            if ($mode -eq 'ControlExplicitTitle') { $arguments += '-ExplicitTitle' }
            elseif ($mode -ne 'Control') { $arguments += "-$mode" }
            Run-Case $backend "gui-$mode" 1 $pwsh $arguments 180
        }
    }
}
$environment = [ordered]@{
    windows = [Environment]::OSVersion.VersionString
    processor_count = [Environment]::ProcessorCount
    powershell = $PSVersionTable.PSVersion.ToString()
    revision = (& git -C "$PSScriptRoot/.." rev-parse HEAD)
    portable_pty = '0.9.0'
    example = $Executable
    gui = $GuiExecutable
    recorded_at = [datetime]::UtcNow.ToString('o')
}
$environment | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'environment.json') -Encoding utf8NoBOM
$failed = @($results | Where-Object { !$_.ok })
function Median([double[]]$Values) {
    if (!$Values.Count) { return $null }
    [array]::Sort($Values)
    $middle = [int][math]::Floor($Values.Count / 2)
    if ($Values.Count % 2) { return $Values[$middle] }
    return ($Values[$middle - 1] + $Values[$middle]) / 2
}
$summary = foreach ($backend in $Backends) {
    $rows = @($results | Where-Object backend -eq $backend)
    $passed = @($rows | Where-Object ok)
    $spawns = @($passed | Where-Object case -eq 'repeat' | ForEach-Object { $_.measurement.metrics.spawn_ms })
    [ordered]@{
        backend = $backend
        checks = $rows.Count
        failures = @($rows | Where-Object { !$_.ok }).Count
        spawn_samples = $spawns.Count
        median_spawn_ms = Median $spawns
        median_bulk_output_ms = Median @($passed | Where-Object case -eq 'bulk-output' | ForEach-Object { $_.measurement.metrics.elapsed_ms })
        median_resize_storm_ms = Median @($passed | Where-Object case -eq 'resize-storm' | ForEach-Object { $_.measurement.metrics.elapsed_ms })
        median_idle_shutdown_ms = Median @($passed | Where-Object case -eq 'idle-close' | ForEach-Object { $_.measurement.metrics.shutdown_ms })
    }
}
$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'summary.json') -Encoding utf8NoBOM
Write-Host "Results: $OutputDirectory ($($results.Count - $failed.Count)/$($results.Count) passed)"
if ($failed.Count) { exit 1 }
