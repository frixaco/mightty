[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

foreach ($architecture in @("x64", "arm64")) {
    $feed = "https://github.com/frixaco/mightty/releases/latest/download/mightty-$architecture.appinstaller"
    $previousPackage = $null
    foreach ($version in @("0.1.0", "0.2.0")) {
        $package = "https://github.com/frixaco/mightty/releases/download/v$version/mightty-$version-$architecture.msix"
        [xml] $manifest = & (Join-Path $PSScriptRoot "new-windows-appinstaller.ps1") `
            -Publisher "CN=Test & Verify" -Version $version -Architecture $architecture `
            -PackageUri $package -AppInstallerUri $feed
        if ($manifest.AppInstaller.Uri -cne $feed -or
            $manifest.AppInstaller.MainPackage.Uri -cne $package -or
            $manifest.AppInstaller.MainPackage.Version -cne "$version.0" -or
            $manifest.AppInstaller.MainPackage.ProcessorArchitecture -cne $architecture -or
            $manifest.AppInstaller.MainPackage.Publisher -cne "CN=Test & Verify") {
            throw "Update feed continuity or manifest identity failed for $version/$architecture."
        }
        if ($null -ne $previousPackage -and $previousPackage -ceq $package) {
            throw "The new release did not advance its package URI."
        }
        $previousPackage = $manifest.AppInstaller.MainPackage.Uri
    }
}
Write-Host "AppInstaller feed continuity and XML escaping passed for x64 and arm64."
