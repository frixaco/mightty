[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $PreviousPackage,

    [Parameter(Mandatory)]
    [string] $CurrentPackage,

    [string] $PackageName = "Mightty.Terminal"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

foreach ($path in @($PreviousPackage, $CurrentPackage)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Package does not exist: $path"
    }
}

$dataDirectory = Join-Path $env:APPDATA "mightty"
if (Test-Path -LiteralPath $dataDirectory) {
    throw "The upgrade test requires a clean Windows user without $dataDirectory."
}

$settingsPath = Join-Path $dataDirectory "settings.json"
$workspaceDirectory = Join-Path $dataDirectory "workspaces"
$workspacePath = Join-Path $workspaceDirectory "upgrade-test.json"
$settings = "{`"app`":{`"sidebar_visible`":false}}"
$workspace = "{`"schema_version`":1,`"upgrade_test`":true}"

try {
    Add-AppxPackage -Path $PreviousPackage
    New-Item -ItemType Directory -Path $workspaceDirectory | Out-Null
    Set-Content -LiteralPath $settingsPath -Value $settings -NoNewline -Encoding utf8
    Set-Content -LiteralPath $workspacePath -Value $workspace -NoNewline -Encoding utf8

    $settingsHash = (Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash
    $workspaceHash = (Get-FileHash -LiteralPath $workspacePath -Algorithm SHA256).Hash
    Add-AppxPackage -Path $CurrentPackage -ForceUpdateFromAnyVersion

    if ((Get-FileHash -LiteralPath $settingsPath -Algorithm SHA256).Hash -ne $settingsHash) {
        throw "The package upgrade changed the user settings file."
    }
    if ((Get-FileHash -LiteralPath $workspacePath -Algorithm SHA256).Hash -ne $workspaceHash) {
        throw "The package upgrade changed the user workspace file."
    }
    Write-Host "The signed package upgrade preserved settings and workspaces."
}
finally {
    Get-AppxPackage -Name $PackageName | Remove-AppxPackage -ErrorAction SilentlyContinue
    $resolvedData = [System.IO.Path]::GetFullPath($dataDirectory)
    $resolvedAppData = [System.IO.Path]::GetFullPath($env:APPDATA).TrimEnd("\") + "\"
    if ($resolvedData.StartsWith($resolvedAppData, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolvedData -Recurse -Force -ErrorAction SilentlyContinue
    }
}
