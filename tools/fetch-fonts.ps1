[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$version = "3.4.0"
$archiveUri = "https://github.com/ryanoasis/nerd-fonts/releases/download/v$version/JetBrainsMono.zip"
$archiveHash = "76f05ff3ace48a464a6ca57977998784ff7bdbb65a6d915d7e401cd3927c493c"
$fontHashes = [ordered]@{
    "JetBrainsMonoNerdFontMono-Regular.ttf" = "F01031F40E48DC29E1112E6B0B0450A2C6CD097F3F35CFFF05C55CB311F8034C"
    "JetBrainsMonoNerdFontMono-Bold.ttf" = "5BDD4A873F3CD32F882D2C55545089123926E27707D5880FC9EAF84EB01B6686"
    "JetBrainsMonoNerdFontMono-Italic.ttf" = "CCD88B36D325E6A905EDC8DD3F2522718D9690D9BED3FBB4684C7E746C34F846"
    "JetBrainsMonoNerdFontMono-BoldItalic.ttf" = "D931DF2928B3216892D35980CDDCAD9EDADE1B9C9CD2E09A6C2937139F474742"
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$fontDirectory = Join-Path $repositoryRoot "fonts\JetBrainsMono"
$fontsAreCurrent = $true
foreach ($entry in $fontHashes.GetEnumerator()) {
    $path = Join-Path $fontDirectory $entry.Key
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        $fontsAreCurrent = $false
        break
    }
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry.Value) {
        $fontsAreCurrent = $false
        break
    }
}
if ($fontsAreCurrent) {
    Write-Host "JetBrainsMono Nerd Font Mono $version is ready."
    exit 0
}

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) (
    "mightty-fonts-" + [guid]::NewGuid().ToString("N")
)
New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
try {
    $archivePath = Join-Path $temporaryDirectory "JetBrainsMono.zip"
    $expandedPath = Join-Path $temporaryDirectory "expanded"
    Invoke-WebRequest -Uri $archiveUri -OutFile $archivePath
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
    if ($actualHash -ne $archiveHash) {
        throw "JetBrainsMono archive hash mismatch. Expected $archiveHash, received $actualHash."
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $expandedPath
    New-Item -ItemType Directory -Force -Path $fontDirectory | Out-Null
    foreach ($entry in $fontHashes.GetEnumerator()) {
        $source = Get-ChildItem -LiteralPath $expandedPath -Recurse -File |
            Where-Object Name -EQ $entry.Key |
            Select-Object -First 1
        if ($null -eq $source) {
            throw "The verified archive does not contain $($entry.Key)."
        }
        if ((Get-FileHash -LiteralPath $source.FullName -Algorithm SHA256).Hash -ne $entry.Value) {
            throw "The verified archive contains an unexpected $($entry.Key)."
        }
        Copy-Item -LiteralPath $source.FullName -Destination (Join-Path $fontDirectory $entry.Key)
    }
}
finally {
    $resolvedTemporary = [System.IO.Path]::GetFullPath($temporaryDirectory)
    $resolvedSystemTemporary = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if ($resolvedTemporary.StartsWith($resolvedSystemTemporary, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolvedTemporary -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "Installed verified JetBrainsMono Nerd Font Mono $version assets."
