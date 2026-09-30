[CmdletBinding()]
param(
    [string] $PackageName = "Mightty.Terminal",
    [Parameter(Mandatory)][string] $Publisher,
    [Parameter(Mandatory)][ValidatePattern("^\d+\.\d+\.\d+$")][string] $Version,
    [ValidateSet("x64", "arm64")][string] $Architecture = "x64",
    [Parameter(Mandatory)][uri] $PackageUri,
    [Parameter(Mandatory)][uri] $AppInstallerUri
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
foreach ($uri in @($PackageUri, $AppInstallerUri)) {
    if (-not $uri.IsAbsoluteUri -or $uri.Scheme -ne "https") {
        throw "Package and update feed URIs must use absolute HTTPS URLs."
    }
}
$template = Get-Content -Raw (Join-Path $PSScriptRoot "..\packaging\windows\Mightty.appinstaller.in")
$values = @{
    PACKAGE_NAME = $PackageName
    PUBLISHER = $Publisher
    VERSION = "$Version.0"
    ARCHITECTURE = $Architecture
    PACKAGE_URI = $PackageUri.AbsoluteUri
    APPINSTALLER_URI = $AppInstallerUri.AbsoluteUri
}
foreach ($entry in $values.GetEnumerator()) {
    $template = $template.Replace("{{$($entry.Key)}}", [Security.SecurityElement]::Escape($entry.Value))
}
$template
