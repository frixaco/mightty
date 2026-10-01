param([string]$Executable = "$PSScriptRoot/../target/debug/mightty.exe", [switch]$UiOnly, [switch]$SnapshotOnly, [switch]$OutputStress, [switch]$Amp, [switch]$PresentationOnly, [ValidateRange(1,3)][int]$AmpPanes = 1)
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
    $controlTimings = [Collections.Generic.List[object]]::new()
    $deadline = [datetime]::UtcNow.AddSeconds(60)
    do {
        if ($OutputStress -or $Amp -or $PresentationOnly) {
            # Read our own descriptor; global discovery probes unrelated stale endpoints.
            $instances = @(Get-ChildItem -LiteralPath (Join-Path $env:LOCALAPPDATA 'mightty/control') -Filter "$($application.Id)-*.json" -ErrorAction SilentlyContinue | ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json })
        } else {
            $instances = (& $Executable ctl instances --json | ConvertFrom-Json).result
        }
        $instance = @($instances | Where-Object pid -eq $application.Id)
        if ($instance.Count -eq 1) { break }
        if ($application.HasExited) { throw 'Isolated instance exited during startup' }
        Start-Sleep -Milliseconds 100
    } while ([datetime]::UtcNow -lt $deadline)
    if ($instance.Count -ne 1) { throw 'Isolated instance was not discovered' }
    function Invoke-Control([string[]]$Command, [int]$TimeoutMs = 0) {
        $separator = [Array]::IndexOf($Command, '--')
        $selectors = @('--instance', $instance[0].instance_id, '--json')
        $controlArguments = if ($separator -ge 1) { $Command[0..($separator - 1)] + $selectors + $Command[$separator..($Command.Length - 1)] } else { $Command + $selectors }
        if ($OutputStress -or $Amp -or $PresentationOnly) {
            $clock = [Diagnostics.Stopwatch]::StartNew()
            # A separate process watchdog works even when the GUI cannot dispatch ctl.
            $start = [Diagnostics.ProcessStartInfo]::new($Executable)
            $start.UseShellExecute = $false
            $start.CreateNoWindow = $true
            $start.RedirectStandardOutput = $true
            $start.RedirectStandardError = $true
            $start.ArgumentList.Add('ctl')
            foreach ($argument in $controlArguments) { $start.ArgumentList.Add($argument) }
            $process = [Diagnostics.Process]::Start($start)
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            # PNG readback/encoding and process startup have separate costs from UI dispatch.
            $timeout = if ($TimeoutMs -gt 0) { $TimeoutMs } elseif ($Command[0] -eq 'snapshot') { 5000 } elseif ($Command[0] -in @('pane', 'tab') -and $Command[1] -in @('split', 'new', 'close')) { 10000 } else { 1000 }
            if (!$process.WaitForExit($timeout)) { $process.Kill($true); $process.Dispose(); throw "Control watchdog: $($Command -join ' '); artifacts: $caseDirectory" }
            $json = $stdout.GetAwaiter().GetResult()
            $errorText = $stderr.GetAwaiter().GetResult()
            $process.Dispose()
            if ([string]::IsNullOrWhiteSpace($json)) { throw "Control returned no JSON: $($Command -join ' '); $errorText; artifacts: $caseDirectory" }
            $response = $json | ConvertFrom-Json
            $controlTimings.Add(@{ command = $Command[0..([Math]::Min(1, $Command.Length - 1))] -join ' '; elapsed_ms = $clock.Elapsed.TotalMilliseconds })
        } else {
            $response = & $Executable ctl @controlArguments | ConvertFrom-Json
        }
        if (!$response.ok) { throw ("Command: $($Command -join ' '); " + ($response.error | ConvertTo-Json -Depth 8) + "; logs: $caseDirectory") }
        $response.result
    }
    # The descriptor is published before the GUI finishes creating its first pane.
    $state = Invoke-Control -Command @('state') -TimeoutMs 10000
    if ($state.windows.Count -ne 1) { throw 'Isolated startup opened unexpected windows' }
    $pane = $state.windows[0].tabs[0].panes[0].pane_id
    if ($PresentationOnly) {
        $null = Invoke-Control @('window','focus','--window','w1')
        $null = Invoke-Control @('snapshot','--window','w1','--frame','next','--out',$caseDirectory)
        $fixture = Join-Path $caseDirectory 'synchronized.ps1'
        @'
$esc = [char]27
[Console]::Write("${esc}[2J${esc}[HCOMMITTED_BASE")
Start-Sleep -Milliseconds 500
[Console]::Write("${esc}[?2026h${esc}[2J${esc}[HHOLD_PENDING${esc}[?25l")
Start-Sleep -Milliseconds 2500
[Console]::Write("${esc}[?2026l${esc}[?25h${esc}[2J${esc}[HRELEASED_FINAL`r`n")
'@ | Set-Content -LiteralPath $fixture -Encoding utf8NoBOM
        @(@{type='text';text="& '$fixture'"},@{type='key';key='enter'}) | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'sync-input.json') -Encoding utf8NoBOM
        $null = Invoke-Control @('pane','input','--pane',$pane,'--file',(Join-Path $caseDirectory 'sync-input.json'))
        $deadline = [datetime]::UtcNow.AddSeconds(10)
        do {
            $held = Invoke-Control @('state','--pane',$pane)
            if ($held.terminal_status.value.synchronized_output) { break }
        } while ([datetime]::UtcNow -lt $deadline)
        if (!$held.terminal_status.value.synchronized_output) { throw 'ConPTY did not deliver synchronized output' }
        $retained = Invoke-Control @('snapshot','--pane',$pane,'--frame','presented','--out',$caseDirectory)
        $retainedFrame = Get-Content -LiteralPath (Join-Path (Split-Path $retained.manifest) 'frame.json') -Raw | ConvertFrom-Json
        if ([long]$retainedFrame.panes[0].state.output_seq -ge [long]$held.output_seq) { throw 'Presented capture published unfinished output' }
        $next = & $Executable ctl snapshot --pane $pane --frame next --out $caseDirectory --timeout 100ms --instance $instance[0].instance_id --json | ConvertFrom-Json
        if ($next.ok -or $next.error.code -ne 'timeout') { throw 'Next capture did not wait through synchronized output' }
        $deadline = [datetime]::UtcNow.AddSeconds(2)
        do { $recovered = Invoke-Control @('state','--pane',$pane) } while ($recovered.terminal_status.value.synchronized_output -and [datetime]::UtcNow -lt $deadline)
        if ($recovered.terminal_status.value.synchronized_output) { throw 'Timeout did not reset the actual mode' }
        $recovery = Invoke-Control @('snapshot','--pane',$pane,'--frame','next','--out',$caseDirectory)
        $recoveryFrame = Get-Content -LiteralPath (Join-Path (Split-Path $recovery.manifest) 'frame.json') -Raw | ConvertFrom-Json
        if ([long]$recoveryFrame.panes[0].state.output_seq -lt [long]$held.output_seq) { throw 'Recovery frame did not include accepted output' }
        $null = Invoke-Control -Command @('wait','--pane',$pane,'--text','RELEASED_FINAL','--timeout','10s') -TimeoutMs 12000
        $null = Invoke-Control @('pane','focus','--pane',$pane)
        $null = Invoke-Control @('ui','text','--window','w1','--text','Write-OutpuX')
        $null = Invoke-Control @('ui','key','--window','w1','--key','backspace')
        $null = Invoke-Control @('ui','text','--window','w1','--text',"t ('editing-' + 'passed')")
        $null = Invoke-Control @('ui','key','--window','w1','--key','enter')
        $editingRead = Invoke-Control @('pane','read','--pane',$pane,'--tail','100')
        $editingRead | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $caseDirectory 'editing-read.json') -Encoding utf8NoBOM
        $null = Invoke-Control -Command @('wait','--pane',$pane,'--text','editing-passed','--timeout','10s') -TimeoutMs 12000
        $editorFile = Join-Path $caseDirectory 'modal.txt'
        $editor = (Get-Command nvim.exe -ErrorAction Stop).Source
        $split = Invoke-Control @('pane','split','--pane',$pane,'--direction','right','--exec',$editor,'--','--clean','-i','NONE',$editorFile)
        $editorPane = $split.new_pane_id
        $deadline = [datetime]::UtcNow.AddSeconds(10)
        do { $editorState=Invoke-Control @('state','--pane',$editorPane) } while ([long]$editorState.output_seq -lt 100 -and [datetime]::UtcNow -lt $deadline)
        $null = Invoke-Control @('pane','focus','--pane',$editorPane)
        $null = Invoke-Control @('ui','key','--window','w1','--key','i')
        $null = Invoke-Control @('ui','text','--window','w1','--text','modal editing passed')
        $null = Invoke-Control @('ui','key','--window','w1','--key','escape')
        $null = Invoke-Control @('ui','text','--window','w1','--text',':wq')
        $null = Invoke-Control @('ui','key','--window','w1','--key','enter')
        $editorRead = Invoke-Control @('pane','read','--pane',$editorPane,'--tail','100')
        $editorRead | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $caseDirectory 'modal-read.json') -Encoding utf8NoBOM
        $null = Invoke-Control -Command @('wait','--pane',$editorPane,'--condition','process-exited','--timeout','10s') -TimeoutMs 12000
        if ((Get-Content -LiteralPath $editorFile -Raw).Trim() -ne 'modal editing passed') { throw 'Modal editor UI input or save failed' }
        Write-Output "Presentation/PowerShell/Neovim passed: $Executable; artifacts: $caseDirectory"
        return
    }
    if ($Amp) {
        $ampExecutable = (Get-Command amp -ErrorAction Stop).Source
        $split = Invoke-Control @('pane', 'split', '--pane', $pane, '--direction', 'right', '--exec', $ampExecutable)
        $ampPane = $split.new_pane_id
        $ampPaneIds = @($ampPane)
        $splitTarget = $ampPane
        for ($n = 1; $n -lt $AmpPanes; $n++) {
            $ratio = (($AmpPanes - $n) / ($AmpPanes - $n + 1.0)).ToString([Globalization.CultureInfo]::InvariantCulture)
            $extra = Invoke-Control @('pane','split','--pane',$splitTarget,'--direction','down','--ratio',$ratio,'--exec',$ampExecutable)
            $ampPaneIds += $extra.new_pane_id
            $splitTarget = $extra.new_pane_id
        }
        $null = Invoke-Control @('window', 'focus', '--window', 'w1')
        $deadline = [datetime]::UtcNow.AddSeconds(20)
        do {
            $ampState = Invoke-Control @('state', '--pane', $ampPane)
            if ([long]$ampState.output_seq -gt 200) { break }
            Start-Sleep -Milliseconds 100
        } while ([datetime]::UtcNow -lt $deadline)
        if ([long]$ampState.output_seq -le 200 -or $ampState.output_eof) { throw 'Amp TUI did not start' }
        $ampBaselines = @{}
        foreach ($id in $ampPaneIds) {
            $ready = Invoke-Control @('state','--pane',$id)
            if ($ready.output_eof) { throw 'An Amp pane exited during startup' }
            $ampBaselines[$id] = [long]$ready.output_seq
        }
        $baseline = [long]$ampState.output_seq
        $started = [datetime]::UtcNow
        $previousFrame = 0L
        while (([datetime]::UtcNow - $started).TotalSeconds -lt 30) {
            # Compose and erase a character to cause redraws without submitting a prompt.
            $null = Invoke-Control @('pane', 'send-key', '--pane', $ampPane, '--key', 'x')
            $null = Invoke-Control @('pane', 'send-key', '--pane', $ampPane, '--key', 'backspace')
            $capture = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
            $manifest = Get-Content -LiteralPath $capture.manifest -Raw | ConvertFrom-Json
            if ([long]$manifest.frame_id -le $previousFrame) { throw 'Amp frame stopped advancing' }
            $previousFrame = [long]$manifest.frame_id
            $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'b', '--mod', 'ctrl')
            $null = Invoke-Control @('window', 'resize', '--window', 'w1', '--width', '1000', '--height', '700')
            $null = Invoke-Control @('window', 'resize', '--window', 'w1', '--width', '1100', '--height', '740')
        }
        $ampState = Invoke-Control @('state', '--pane', $ampPane)
        if ([long]$ampState.output_seq -le $baseline -or $ampState.output_eof) { throw 'Amp redraw output stopped' }
        $geometry = Invoke-Control @('state', '--tab', $split.tab_id)
        $left = @($geometry.panes | Where-Object pane_id -eq $pane)[0].computed_bounds
        $x = $left.x + $left.width + 2
        $y = $left.y + $left.height / 2
        $drag = @(@{type='pointer';event='move';x=$x;y=$y},@{type='pointer';event='press';button='left';x=$x;y=$y})
        $moves = if ($AmpPanes -gt 1) {60} else {2}
        for ($move = 1; $move -le $moves; $move++) { $drag += @{type='pointer';event='move';x=$x+20*$move/$moves;y=$y} }
        $drag += @{type='pointer';event='release';button='left';x=$x+20;y=$y}
        $drag | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'amp-drag.json') -Encoding utf8NoBOM
        # The long drag schedules 63 steps; retain one-second watchdogs for single UI operations.
        $dragTimeout = if ($AmpPanes -gt 1) {10000} else {1000}
        $null = Invoke-Control -Command @('ui', 'input', '--window', 'w1', '--file', (Join-Path $caseDirectory 'amp-drag.json')) -TimeoutMs $dragTimeout
        if ((Invoke-Control @('state', '--tab', $split.tab_id)).layout_token -eq $geometry.layout_token) { throw 'Amp divider drag did not change layout' }
        $ampState | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $caseDirectory 'amp-state.json') -Encoding utf8NoBOM
        foreach ($id in $ampPaneIds) {
            $busy = Invoke-Control @('state','--pane',$id)
            if ($busy.output_eof -or [long]$busy.output_seq -le $ampBaselines[$id]) { throw 'An Amp pane stopped' }
            $null = Invoke-Control @('pane', 'close', '--pane', $id)
        }
        Write-Output "Amp TUI redraw/input/resize passed: $Executable; artifacts: $caseDirectory"
        return
    }
    if ($OutputStress) {
        $producer = Join-Path $caseDirectory 'producer.ps1'
        @'
param([string]$StopFile, [string]$ResultFile)
$stream = [Console]::OpenStandardOutput()
$esc = [char]27
$plain = ('ordinary-output-' + ('x' * 60) + "`r`n") * 16
$frame = "$esc[H$esc]8;;https://example.com/$esc\hyperlink$esc]8;;$esc\`r`n" + $plain
$bytes = [Text.Encoding]::UTF8.GetBytes($frame)
$begin = [Text.Encoding]::UTF8.GetBytes("$esc[?1049h")
$stream.Write($begin)
$total = $begin.Length
$deadline = [datetime]::UtcNow.AddMinutes(2)
while (!(Test-Path -LiteralPath $StopFile) -and [datetime]::UtcNow -lt $deadline) {
    $stream.Write($bytes)
    $total += $bytes.Length
}
$end = [Text.Encoding]::UTF8.GetBytes("$esc[?1049l`r`nOUTPUT-STRESS-FINAL`r`n")
$stream.Write($end)
$stream.Flush()
@{ bytes = $total + $end.Length } | ConvertTo-Json | Set-Content -LiteralPath $ResultFile
'@ | Set-Content -LiteralPath $producer -Encoding utf8NoBOM
        $stop = Join-Path $caseDirectory 'stop-output'
        $busy = [Collections.Generic.List[string]]::new()
        $split = Invoke-Control @('pane', 'split', '--pane', $pane, '--direction', 'right', '--exec', 'pwsh.exe', '--', '-NoLogo', '-NoProfile', '-File', $producer, $stop, (Join-Path $caseDirectory 'producer-1.json'))
        $busy.Add($split.new_pane_id)
        $null = Invoke-Control @('window', 'focus', '--window', 'w1')
        $samples = [Collections.Generic.List[object]]::new()
        $started = [datetime]::UtcNow
        $previousFrame = 0L
        $previousBytes = @{}
        $expanded = $false
        while (([datetime]::UtcNow - $started).TotalSeconds -lt 30) {
            foreach ($id in $busy) {
                $observed = Invoke-Control @('state', '--pane', $id)
                $count = [long]$observed.output_seq
                if ($previousBytes.ContainsKey($id) -and $count -le $previousBytes[$id]) { throw "Output stopped advancing in $id" }
                $previousBytes[$id] = $count
            }
            # Exercise input and a new frame before the first resize.
            $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'b', '--mod', 'ctrl')
            $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'p', '--mod', 'ctrl', '--mod', 'shift')
            $null = Invoke-Control @('ui', 'key', '--window', 'w1', '--key', 'escape')
            $capture = Invoke-Control @('snapshot', '--window', 'w1', '--frame', 'next', '--out', $caseDirectory)
            $manifest = Get-Content -LiteralPath $capture.manifest -Raw | ConvertFrom-Json
            if ([long]$manifest.frame_id -le $previousFrame) { throw 'Presented frame did not advance during output' }
            $previousFrame = [long]$manifest.frame_id
            $samples.Add(@{ elapsed_ms = ([datetime]::UtcNow - $started).TotalMilliseconds; frame = $manifest; output = $previousBytes.Clone() })
            $null = Invoke-Control @('window', 'resize', '--window', 'w1', '--width', '1000', '--height', '700')
            $null = Invoke-Control @('window', 'resize', '--window', 'w1', '--width', '1100', '--height', '740')
            if (!$expanded -and ([datetime]::UtcNow - $started).TotalSeconds -ge 10) {
                $geometry = Invoke-Control @('state', '--tab', $split.tab_id)
                $left = @($geometry.panes | Where-Object pane_id -eq $pane)[0].computed_bounds
                $x = $left.x + $left.width + 2
                $y = $left.y + $left.height / 2
                @(@{ type = 'pointer'; event = 'move'; x = $x; y = $y }, @{ type = 'pointer'; event = 'press'; button = 'left'; x = $x; y = $y }, @{ type = 'pointer'; event = 'move'; x = $x + 8; y = $y }, @{ type = 'pointer'; event = 'move'; x = $x + 20; y = $y }, @{ type = 'pointer'; event = 'release'; button = 'left'; x = $x + 20; y = $y }) | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $caseDirectory 'stress-drag.json') -Encoding utf8NoBOM
                $null = Invoke-Control @('ui', 'input', '--window', 'w1', '--file', (Join-Path $caseDirectory 'stress-drag.json'))
                if ((Invoke-Control @('state', '--tab', $split.tab_id)).layout_token -eq $geometry.layout_token) { throw 'Stress divider drag did not change layout' }
                $idleTab = Invoke-Control @('tab', 'new', '--window', 'w1', '--profile', 'fixture')
                $null = Invoke-Control @('tab', 'select', '--tab', $idleTab.tab_id)
                $null = Invoke-Control @('tab', 'select', '--tab', $split.tab_id)
                $null = Invoke-Control @('tab', 'close', '--tab', $idleTab.tab_id)
                for ($index = 2; $index -le 3; $index++) {
                    $extra = Invoke-Control @('pane', 'split', '--pane', $pane, '--direction', 'down', '--exec', 'pwsh.exe', '--', '-NoLogo', '-NoProfile', '-File', $producer, $stop, (Join-Path $caseDirectory "producer-$index.json"))
                    $busy.Add($extra.new_pane_id)
                }
                $expanded = $true
            }
        }
        # Closing an active producer must also be interruptible.
        $null = Invoke-Control @('pane', 'close', '--pane', $busy[2])
        New-Item -ItemType File -Path $stop | Out-Null
        $deadline = [datetime]::UtcNow.AddSeconds(20)
        foreach ($id in @($busy[0], $busy[1])) {
            do {
                $final = Invoke-Control @('state', '--pane', $id)
                if ($final.removed_at) { break }
                Start-Sleep -Milliseconds 100
            } while ([datetime]::UtcNow -lt $deadline)
            if (!$final.removed_at -or !$final.output_eof -or !$final.final_tail.Contains('OUTPUT-STRESS-FINAL') -or [long]$final.output_seq -lt $previousBytes[$id]) { throw "Final output missing for $id" }
        }
        $samples | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $caseDirectory 'output-stress.json') -Encoding utf8NoBOM
        Write-Output "Sustained output passed: $Executable; artifacts: $caseDirectory"
        return
    }
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
        # Search paints interactive children; a scratch capture must provide a view context.
        $null = Invoke-Control @('ui', 'search', '--pane', $backgroundPane, '--open', 'true', '--query', 'flower')
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
    if ($OutputStress -or $Amp -or $PresentationOnly) { $controlTimings | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $caseDirectory 'control-timings.json') -Encoding utf8NoBOM }
    if (!$application.HasExited) { Stop-Process -Id $application.Id }
}
