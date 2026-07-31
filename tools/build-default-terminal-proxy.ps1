[CmdletBinding()]
param(
    [ValidateSet("x64", "arm64")]
    [string] $Architecture = "x64",

    [Parameter(Mandatory)]
    [string] $OutputPath
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
        throw "Install the Windows SDK. The build requires $Name."
    }
    $tool
}

function Import-VisualCppEnvironment {
    param([Parameter(Mandatory)][string] $TargetArchitecture)

    $vsWhere = Join-Path ${env:ProgramFiles(x86)} `
        "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vsWhere -PathType Leaf)) {
        throw "Install Visual Studio Build Tools with the C++ workload."
    }
    $installation = & $vsWhere `
        -latest `
        -products "*" `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($installation)) {
        throw "Visual Studio Build Tools with the C++ workload were not found."
    }

    $vcVarsAll = Join-Path $installation "VC\Auxiliary\Build\vcvarsall.bat"
    $vcTarget = if ($TargetArchitecture -eq "arm64") {
        "amd64_arm64"
    }
    else {
        "amd64"
    }
    $command = "call `"$vcVarsAll`" $vcTarget >nul && set"
    $environment = & $env:COMSPEC /d /s /c $command
    if ($LASTEXITCODE -ne 0) {
        throw "Visual Studio C++ environment setup failed."
    }
    foreach ($line in $environment) {
        $separator = $line.IndexOf("=")
        if ($separator -le 0) {
            continue
        }
        [Environment]::SetEnvironmentVariable(
            $line.Substring(0, $separator),
            $line.Substring($separator + 1),
            "Process"
        )
    }
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$buildParent = Join-Path $repositoryRoot "target\default-terminal-proxy"
$buildDirectory = Join-Path $buildParent $Architecture
$safeBuildParent = [IO.Path]::GetFullPath($buildParent).TrimEnd("\") + "\"
$resolvedBuildDirectory = [IO.Path]::GetFullPath($buildDirectory)
if (-not $resolvedBuildDirectory.StartsWith(
    $safeBuildParent,
    [StringComparison]::OrdinalIgnoreCase
)) {
    throw "The proxy build path is outside the expected target directory."
}
if (Test-Path -LiteralPath $resolvedBuildDirectory) {
    Remove-Item -LiteralPath $resolvedBuildDirectory -Recurse -Force
}
New-Item -ItemType Directory -Path $resolvedBuildDirectory | Out-Null

Import-VisualCppEnvironment $Architecture
$midl = Find-WindowsSdkTool "midl.exe"
$idl = Join-Path $repositoryRoot `
    "packaging\windows\default-terminal\ITerminalHandoff.idl"
& $midl `
    /nologo `
    /env $Architecture `
    /robust `
    /target NT100 `
    /out $resolvedBuildDirectory `
    /h ITerminalHandoff.h `
    /iid ITerminalHandoff_i.c `
    /proxy ITerminalHandoff_p.c `
    /dlldata dlldata.c `
    $idl
if ($LASTEXITCODE -ne 0) {
    throw "MIDL failed to generate the default-terminal proxy."
}

$compiler = (Get-Command cl.exe -ErrorAction Stop).Source
$linker = (Get-Command link.exe -ErrorAction Stop).Source
$proxyClsid = "PROXY_CLSID_IS={0xE22C363C,0x12F1,0x483B,{0xA3,0xF0,0xE9,0x7F,0x31,0xAE,0xEF,0xCB}}"
$machine = if ($Architecture -eq "arm64") { "ARM64" } else { "X64" }
$definition = Join-Path $repositoryRoot `
    "packaging\windows\default-terminal\MighttyTerminalProxy.def"
$builtProxy = Join-Path $resolvedBuildDirectory "MighttyTerminalProxy.dll"

Push-Location $resolvedBuildDirectory
try {
    & $compiler `
        /nologo `
        /c `
        /O2 `
        /MT `
        /W4 `
        /WX `
        /DWIN32 `
        /D_WIN32_WINNT=0x0A00 `
        /DREGISTER_PROXY_DLL `
        "/D$proxyClsid" `
        dlldata.c `
        ITerminalHandoff_i.c `
        ITerminalHandoff_p.c
    if ($LASTEXITCODE -ne 0) {
        throw "The C compiler failed to build the default-terminal proxy."
    }

    & $linker `
        /nologo `
        /dll `
        "/machine:$machine" `
        "/def:$definition" `
        "/out:$builtProxy" `
        dlldata.obj `
        ITerminalHandoff_i.obj `
        ITerminalHandoff_p.obj `
        rpcrt4.lib `
        ole32.lib `
        oleaut32.lib
    if ($LASTEXITCODE -ne 0) {
        throw "The linker failed to build the default-terminal proxy."
    }
}
finally {
    Pop-Location
}

$resolvedOutputPath = [IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path (
    Split-Path -Parent $resolvedOutputPath
) | Out-Null
Copy-Item -LiteralPath $builtProxy -Destination $resolvedOutputPath -Force
Write-Host "Built default-terminal proxy $resolvedOutputPath"
