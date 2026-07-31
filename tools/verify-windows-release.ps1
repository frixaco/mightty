[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $ArtifactsDirectory,

    [Parameter(Mandatory)]
    [string] $Publisher,

    [Parameter(Mandatory)]
    [ValidatePattern("^\d+\.\d+\.\d+$")]
    [string] $Version,

    [ValidateSet("x64", "arm64")]
    [string] $Architecture = "x64",

    [string] $PackageName = "Mightty.Terminal"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Find-WindowsSdkTool {
    param([Parameter(Mandatory)][string] $Name)

    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $tool = Get-ChildItem -LiteralPath $sdkRoot -Directory |
        Where-Object Name -Match "^\d" |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1
    if ($null -eq $tool) {
        throw "Install the Windows SDK. Release verification requires $Name."
    }
    $tool
}

function Require-Equal {
    param(
        [Parameter(Mandatory)][string] $Actual,
        [Parameter(Mandatory)][string] $Expected,
        [Parameter(Mandatory)][string] $Field
    )

    if ($Actual -cne $Expected) {
        throw "$Field is '$Actual'. Expected '$Expected'."
    }
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$resolvedArtifacts = [IO.Path]::GetFullPath($ArtifactsDirectory)
$packageFileName = "mightty-$Version-$Architecture.msix"
$packagePath = Join-Path $resolvedArtifacts $packageFileName
$appInstallerPath = Join-Path $resolvedArtifacts "mightty.appinstaller"
foreach ($path in @($packagePath, $appInstallerPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Release artifact does not exist: $path"
    }
}
$packages = @(Get-ChildItem -LiteralPath $resolvedArtifacts -Filter "*.msix" -File)
if ($packages.Count -ne 1 -or $packages[0].Name -cne $packageFileName) {
    throw "The release must contain only $packageFileName."
}

$signTool = Find-WindowsSdkTool "SignTool.exe"
& $signTool verify /pa /v $packagePath
if ($LASTEXITCODE -ne 0) {
    throw "The release package signature is not valid."
}

$verificationParent = Join-Path $repositoryRoot "target\windows-release-verification"
$verificationDirectory = Join-Path $verificationParent $Architecture
$safeParent = [IO.Path]::GetFullPath($verificationParent).TrimEnd("\") + "\"
$resolvedVerification = [IO.Path]::GetFullPath($verificationDirectory)
if (-not $resolvedVerification.StartsWith(
    $safeParent,
    [StringComparison]::OrdinalIgnoreCase
)) {
    throw "The verification path is outside the expected target directory."
}

if (Test-Path -LiteralPath $resolvedVerification) {
    Remove-Item -LiteralPath $resolvedVerification -Recurse -Force
}
New-Item -ItemType Directory -Path $resolvedVerification | Out-Null

try {
    $makeAppx = Find-WindowsSdkTool "MakeAppx.exe"
    & $makeAppx unpack /o /p $packagePath /d $resolvedVerification
    if ($LASTEXITCODE -ne 0) {
        throw "MakeAppx could not unpack the release package."
    }

    $manifestPath = Join-Path $resolvedVerification "AppxManifest.xml"
    [xml] $manifest = Get-Content -Raw -LiteralPath $manifestPath
    $manifestNamespaces = [Xml.XmlNamespaceManager]::new($manifest.NameTable)
    $manifestNamespaces.AddNamespace(
        "f",
        "http://schemas.microsoft.com/appx/manifest/foundation/windows10"
    )
    $manifestNamespaces.AddNamespace(
        "uap3",
        "http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
    )
    $manifestNamespaces.AddNamespace(
        "com",
        "http://schemas.microsoft.com/appx/manifest/com/windows10"
    )
    $identity = $manifest.SelectSingleNode(
        "/f:Package/f:Identity",
        $manifestNamespaces
    )
    if ($null -eq $identity) {
        throw "The package manifest does not contain an identity."
    }
    Require-Equal `
        -Actual $identity.GetAttribute("Name") `
        -Expected $PackageName `
        -Field "Package name"
    Require-Equal `
        -Actual $identity.GetAttribute("Publisher") `
        -Expected $Publisher `
        -Field "Package publisher"
    Require-Equal `
        -Actual $identity.GetAttribute("Version") `
        -Expected "$Version.0" `
        -Field "Package version"
    Require-Equal `
        -Actual $identity.GetAttribute("ProcessorArchitecture") `
        -Expected $Architecture `
        -Field "Package architecture"

    $terminalExtension = $manifest.SelectSingleNode(
        "/f:Package/f:Applications/f:Application/f:Extensions/uap3:Extension/" +
            "uap3:AppExtension[@Name='com.microsoft.windows.terminal.host']",
        $manifestNamespaces
    )
    $terminalClass = $manifest.SelectSingleNode(
        "/f:Package/f:Applications/f:Application/f:Extensions/com:Extension/" +
            "com:ComServer/com:ExeServer/com:Class",
        $manifestNamespaces
    )
    if ($null -eq $terminalExtension -or $null -eq $terminalClass) {
        throw "The package does not contain the default-terminal registrations."
    }
    $terminalClsidNode = $terminalExtension.SelectSingleNode(
        "uap3:Properties/f:Clsid",
        $manifestNamespaces
    )
    if ($null -eq $terminalClsidNode) {
        throw "The default-terminal extension does not identify its COM class."
    }
    $terminalClsid = "{D4725759-69BD-469F-9819-F27E6C135ED5}"
    Require-Equal `
        -Actual $terminalClsidNode.InnerText `
        -Expected $terminalClsid `
        -Field "Default-terminal extension class"
    Require-Equal `
        -Actual $terminalClass.GetAttribute("Id") `
        -Expected $terminalClsid.Trim("{}") `
        -Field "Default-terminal COM class"

    foreach ($payload in @(
        "mightty.exe",
        "MighttyTerminalProxy.dll",
        "THIRD_PARTY_NOTICES.md"
    )) {
        $payloadPath = Join-Path $resolvedVerification $payload
        if (-not (Test-Path -LiteralPath $payloadPath -PathType Leaf) -or
            (Get-Item -LiteralPath $payloadPath).Length -eq 0) {
            throw "The package payload is missing or empty: $payload"
        }
    }

    [xml] $appInstaller = Get-Content -Raw -LiteralPath $appInstallerPath
    $installerNamespaces = [Xml.XmlNamespaceManager]::new($appInstaller.NameTable)
    $installerNamespaces.AddNamespace(
        "a",
        "http://schemas.microsoft.com/appx/appinstaller/2021"
    )
    $mainPackage = $appInstaller.SelectSingleNode(
        "/a:AppInstaller/a:MainPackage",
        $installerNamespaces
    )
    if ($null -eq $mainPackage) {
        throw "The updater manifest does not contain a main package."
    }
    Require-Equal `
        -Actual $mainPackage.GetAttribute("Name") `
        -Expected $PackageName `
        -Field "Updater package name"
    Require-Equal `
        -Actual $mainPackage.GetAttribute("Publisher") `
        -Expected $Publisher `
        -Field "Updater publisher"
    Require-Equal `
        -Actual $mainPackage.GetAttribute("Version") `
        -Expected "$Version.0" `
        -Field "Updater version"
    Require-Equal `
        -Actual $mainPackage.GetAttribute("ProcessorArchitecture") `
        -Expected $Architecture `
        -Field "Updater architecture"
    Require-Equal `
        -Actual $appInstaller.DocumentElement.GetAttribute("Version") `
        -Expected "$Version.0" `
        -Field "Updater manifest version"
    $appInstallerUri = [uri] $appInstaller.DocumentElement.GetAttribute("Uri")
    if (-not $appInstallerUri.IsAbsoluteUri -or
        [IO.Path]::GetFileName($appInstallerUri.AbsolutePath) -cne
            "mightty.appinstaller") {
        throw "The updater manifest URI does not identify mightty.appinstaller."
    }
    $packageUri = [uri] $mainPackage.GetAttribute("Uri")
    if (-not $packageUri.IsAbsoluteUri -or
        [IO.Path]::GetFileName($packageUri.AbsolutePath) -cne $packageFileName) {
        throw "The updater package URI does not identify $packageFileName."
    }

    $hashes = @($packagePath, $appInstallerPath) | ForEach-Object {
        $file = Get-Item -LiteralPath $_
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash *$($file.Name)"
    }
    Set-Content `
        -LiteralPath (Join-Path $resolvedArtifacts "SHA256SUMS.txt") `
        -Value $hashes `
        -Encoding ascii
}
finally {
    if (Test-Path -LiteralPath $resolvedVerification) {
        Remove-Item -LiteralPath $resolvedVerification -Recurse -Force
    }
}

Write-Host "Verified signed Windows release artifacts without a private certificate."
