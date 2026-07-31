[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $Publisher,

    [Parameter(Mandatory)]
    [ValidatePattern("^[0-9A-Fa-f]{40}$")]
    [string] $CertificateThumbprint,

    [ValidateSet("x64", "arm64")]
    [string] $Architecture = "x64",

    [string] $PackageName = "Mightty.Terminal",

    [string] $ReleaseBaseUri,

    [uri] $TimestampUri = "http://timestamp.digicert.com",

    [string] $OutputDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Find-WindowsSdkTool {
    param([Parameter(Mandatory)][string] $Name)

    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $tool = Get-ChildItem -LiteralPath $sdkRoot -Directory |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1
    if ($null -eq $tool) {
        throw "Install the Windows SDK. The build requires $Name."
    }
    $tool
}

function Write-PackageIcon {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][int] $Width,
        [Parameter(Mandatory)][int] $Height
    )

    Add-Type -AssemblyName System.Drawing
    $bitmap = [System.Drawing.Bitmap]::new($Width, $Height)
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear([System.Drawing.Color]::FromArgb(17, 19, 24))
            $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
            $penWidth = [Math]::Max(2, [Math]::Round([Math]::Min($Width, $Height) * 0.09))
            $pen = [System.Drawing.Pen]::new(
                [System.Drawing.Color]::FromArgb(124, 231, 178),
                $penWidth
            )
            try {
                $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
                $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
                $marginX = $Width * 0.22
                $top = $Height * 0.24
                $bottom = $Height * 0.76
                $center = $Width * 0.5
                $graphics.DrawLines($pen, [System.Drawing.PointF[]]@(
                    [System.Drawing.PointF]::new($marginX, $bottom),
                    [System.Drawing.PointF]::new($marginX, $top),
                    [System.Drawing.PointF]::new($center, $bottom * 0.78),
                    [System.Drawing.PointF]::new($Width - $marginX, $top),
                    [System.Drawing.PointF]::new($Width - $marginX, $bottom)
                ))
            }
            finally {
                $pen.Dispose()
            }
        }
        finally {
            $graphics.Dispose()
        }
        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $bitmap.Dispose()
    }
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$cargoManifest = Get-Content -Raw (Join-Path $repositoryRoot "Cargo.toml")
$versionMatch = [regex]::Match(
    $cargoManifest,
    '(?ms)^\[package\].*?^version\s*=\s*"(?<version>\d+\.\d+\.\d+)"'
)
if (-not $versionMatch.Success) {
    throw "Cargo.toml does not contain a three-part package version."
}
$version = $versionMatch.Groups["version"].Value
$manifestVersion = "$version.0"
if ([string]::IsNullOrWhiteSpace($ReleaseBaseUri)) {
    $ReleaseBaseUri = "https://github.com/frixaco/mightty/releases/download/v$version"
}
$ReleaseBaseUri = $ReleaseBaseUri.TrimEnd("/")
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repositoryRoot "artifacts\windows"
}

$certificate = Get-Item -LiteralPath "Cert:\CurrentUser\My\$CertificateThumbprint"
if (-not $certificate.HasPrivateKey) {
    throw "The signing certificate does not contain a private key."
}
if ($certificate.Subject -ne $Publisher) {
    throw "The certificate subject must equal the package publisher '$Publisher'."
}
if ($certificate.NotBefore -gt [datetime]::Now -or $certificate.NotAfter -lt [datetime]::Now) {
    throw "The signing certificate is not currently valid."
}
$codeSigningOid = "1.3.6.1.5.5.7.3.3"
if ($certificate.EnhancedKeyUsageList.Count -gt 0 -and
    $certificate.EnhancedKeyUsageList.ObjectId -notcontains $codeSigningOid) {
    throw "The signing certificate is not valid for code signing."
}

& (Join-Path $PSScriptRoot "fetch-fonts.ps1")
if ($LASTEXITCODE -ne 0) {
    throw "Font preparation failed."
}

$target = if ($Architecture -eq "x64") {
    "x86_64-pc-windows-msvc"
}
else {
    "aarch64-pc-windows-msvc"
}
& mise exec -- cargo build --locked --release --target $target
if ($LASTEXITCODE -ne 0) {
    throw "The release build failed."
}

$executable = Join-Path $repositoryRoot "target\$target\release\mightty.exe"
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "The release build did not create $executable."
}

$stageRoot = Join-Path $repositoryRoot "target\package-windows\$Architecture"
$safeStageParent = [System.IO.Path]::GetFullPath(
    (Join-Path $repositoryRoot "target\package-windows")
).TrimEnd("\") + "\"
$resolvedStageRoot = [System.IO.Path]::GetFullPath($stageRoot)
if (-not $resolvedStageRoot.StartsWith(
    $safeStageParent,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw "The package stage path is outside the expected target directory."
}
if (Test-Path -LiteralPath $resolvedStageRoot) {
    Remove-Item -LiteralPath $resolvedStageRoot -Recurse -Force
}
$assetDirectory = Join-Path $resolvedStageRoot "Assets"
New-Item -ItemType Directory -Path $assetDirectory | Out-Null
Copy-Item -LiteralPath $executable -Destination (Join-Path $resolvedStageRoot "mightty.exe")
Copy-Item `
    -LiteralPath (Join-Path $repositoryRoot "THIRD_PARTY_NOTICES.md") `
    -Destination (Join-Path $resolvedStageRoot "THIRD_PARTY_NOTICES.md")
& (Join-Path $PSScriptRoot "build-default-terminal-proxy.ps1") `
    -Architecture $Architecture `
    -OutputPath (Join-Path $resolvedStageRoot "MighttyTerminalProxy.dll")
if ($LASTEXITCODE -ne 0) {
    throw "The default-terminal proxy build failed."
}

$xmlPublisher = [System.Security.SecurityElement]::Escape($Publisher)
$manifestTemplate = Get-Content -Raw (
    Join-Path $repositoryRoot "packaging\windows\AppxManifest.xml.in"
)
$manifest = $manifestTemplate.
    Replace("{{PACKAGE_NAME}}", $PackageName).
    Replace("{{PUBLISHER}}", $xmlPublisher).
    Replace("{{VERSION}}", $manifestVersion).
    Replace("{{ARCHITECTURE}}", $Architecture)
Set-Content -LiteralPath (Join-Path $resolvedStageRoot "AppxManifest.xml") -Value $manifest -Encoding utf8

Write-PackageIcon (Join-Path $assetDirectory "StoreLogo.png") 50 50
Write-PackageIcon (Join-Path $assetDirectory "Square44x44Logo.png") 44 44
Write-PackageIcon (Join-Path $assetDirectory "Square150x150Logo.png") 150 150
Write-PackageIcon (Join-Path $assetDirectory "Wide310x150Logo.png") 310 150

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$packageFileName = "mightty-$version-$Architecture.msix"
$packagePath = Join-Path $OutputDirectory $packageFileName
$appInstallerPath = Join-Path $OutputDirectory "mightty.appinstaller"
if (Test-Path -LiteralPath $packagePath) {
    Remove-Item -LiteralPath $packagePath -Force
}

$makeAppx = Find-WindowsSdkTool "MakeAppx.exe"
$signTool = Find-WindowsSdkTool "SignTool.exe"
& $makeAppx pack /o /h SHA256 /d $resolvedStageRoot /p $packagePath
if ($LASTEXITCODE -ne 0) {
    throw "MakeAppx failed."
}
& $signTool sign /fd SHA256 /sha1 $CertificateThumbprint /tr $TimestampUri.AbsoluteUri /td SHA256 $packagePath
if ($LASTEXITCODE -ne 0) {
    throw "SignTool failed."
}
& $signTool verify /pa /v $packagePath
if ($LASTEXITCODE -ne 0) {
    throw "The signed package failed verification."
}

$packageUri = "$ReleaseBaseUri/$packageFileName"
$appInstallerUri = "$ReleaseBaseUri/mightty.appinstaller"
$installerTemplate = Get-Content -Raw (
    Join-Path $repositoryRoot "packaging\windows\Mightty.appinstaller.in"
)
$installer = $installerTemplate.
    Replace("{{PACKAGE_NAME}}", $PackageName).
    Replace("{{PUBLISHER}}", $xmlPublisher).
    Replace("{{VERSION}}", $manifestVersion).
    Replace("{{ARCHITECTURE}}", $Architecture).
    Replace("{{PACKAGE_URI}}", $packageUri).
    Replace("{{APPINSTALLER_URI}}", $appInstallerUri)
Set-Content -LiteralPath $appInstallerPath -Value $installer -Encoding utf8

Write-Host "Created and verified $packagePath"
Write-Host "Created updater manifest $appInstallerPath"
