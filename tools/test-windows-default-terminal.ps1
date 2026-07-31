[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $Package,

    [string] $PackageName = "Mightty.Terminal"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Package -PathType Leaf)) {
    throw "Package does not exist: $Package"
}
if (Get-AppxPackage -Name $PackageName) {
    throw "The default-terminal test requires a user without $PackageName installed."
}
if (Get-Process -Name "mightty" -ErrorAction SilentlyContinue) {
    throw "Close all mightty processes before the default-terminal test."
}

$registryPath = "Console\%%Startup"
$consoleName = "DelegationConsole"
$terminalName = "DelegationTerminal"
$consoleClsid = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}"
$terminalClsid = "{D4725759-69BD-469F-9819-F27E6C135ED5}"
$registryKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey(
    $registryPath,
    $true
)
$createdRegistryKey = $null -eq $registryKey
if ($createdRegistryKey) {
    $registryKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey(
        $registryPath,
        $true
    )
}
$valueNames = $registryKey.GetValueNames()
$consoleExisted = $valueNames -contains $consoleName
$terminalExisted = $valueNames -contains $terminalName
$consoleValue = if ($consoleExisted) {
    $registryKey.GetValue(
        $consoleName,
        $null,
        [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    )
}
$terminalValue = if ($terminalExisted) {
    $registryKey.GetValue(
        $terminalName,
        $null,
        [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    )
}
$consoleKind = if ($consoleExisted) {
    $registryKey.GetValueKind($consoleName)
}
$terminalKind = if ($terminalExisted) {
    $registryKey.GetValueKind($terminalName)
}

$installedPackage = $null
$mighttyProcess = $null
try {
    Add-AppxPackage -Path $Package
    $installedPackage = Get-AppxPackage -Name $PackageName
    if ($null -eq $installedPackage) {
        throw "Windows did not install $PackageName."
    }

    $registryKey.SetValue(
        $consoleName,
        $consoleClsid,
        [Microsoft.Win32.RegistryValueKind]::String
    )
    $registryKey.SetValue(
        $terminalName,
        $terminalClsid,
        [Microsoft.Win32.RegistryValueKind]::String
    )

    $startedAt = Get-Date
    $client = Start-Process `
        -FilePath "$env:SystemRoot\System32\cmd.exe" `
        -ArgumentList @(
            "/d",
            "/q",
            "/c",
            "echo mightty default terminal smoke test & ping -n 3 127.0.0.1 > nul"
        ) `
        -PassThru
    $deadline = (Get-Date).AddSeconds(15)
    do {
        $mighttyProcess = Get-CimInstance Win32_Process |
            Where-Object {
                $_.Name -eq "mightty.exe" -and
                $_.CreationDate -ge $startedAt.AddSeconds(-1) -and
                $_.CommandLine -match "(?i)[-/]Embedding"
            } |
            Select-Object -First 1
        if ($null -eq $mighttyProcess) {
            Start-Sleep -Milliseconds 200
        }
    } while ($null -eq $mighttyProcess -and (Get-Date) -lt $deadline)

    $client.WaitForExit()
    if ($null -eq $mighttyProcess) {
        throw "Windows did not start the mightty terminal handoff server."
    }

    $windowProcess = Get-Process -Id $mighttyProcess.ProcessId
    do {
        $windowProcess.Refresh()
        if ($windowProcess.MainWindowHandle -ne [IntPtr]::Zero) {
            break
        }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $deadline)
    if ($windowProcess.MainWindowHandle -eq [IntPtr]::Zero) {
        throw "mightty accepted the handoff but did not show a terminal window."
    }

    Write-Host "The packaged default-terminal handoff opened a mightty window."
}
finally {
    if ($consoleExisted) {
        $registryKey.SetValue($consoleName, $consoleValue, $consoleKind)
    }
    else {
        $registryKey.DeleteValue($consoleName, $false)
    }
    if ($terminalExisted) {
        $registryKey.SetValue($terminalName, $terminalValue, $terminalKind)
    }
    else {
        $registryKey.DeleteValue($terminalName, $false)
    }
    $registryKey.Dispose()

    if ($createdRegistryKey) {
        $emptyKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey(
            $registryPath
        )
        $isEmpty = $null -ne $emptyKey -and
            $emptyKey.GetValueNames().Count -eq 0 -and
            $emptyKey.GetSubKeyNames().Count -eq 0
        if ($null -ne $emptyKey) {
            $emptyKey.Dispose()
        }
        if ($isEmpty) {
            [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey(
                $registryPath,
                $false
            )
        }
    }

    if ($null -ne $mighttyProcess) {
        $createdProcess = Get-CimInstance `
            -ClassName Win32_Process `
            -Filter "ProcessId = $($mighttyProcess.ProcessId)" `
            -ErrorAction SilentlyContinue
        if ($null -ne $createdProcess -and
            $createdProcess.Name -eq "mightty.exe" -and
            $createdProcess.CreationDate -ge $startedAt.AddSeconds(-1)) {
            Stop-Process `
                -Id $createdProcess.ProcessId `
                -Force `
                -ErrorAction SilentlyContinue
        }
    }
    if ($null -ne $installedPackage) {
        Remove-AppxPackage `
            -Package $installedPackage.PackageFullName `
            -ErrorAction SilentlyContinue
    }
}
