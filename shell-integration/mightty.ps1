# mightty shell integration for interactive PowerShell sessions.

if ($global:__MighttyShellIntegrationLoaded) {
    return
}
$global:__MighttyShellIntegrationLoaded = $true
$global:__MighttyCommandPending = $false
$script:MighttySemanticIntegrationAvailable = $false

$script:MighttyOriginalPrompt = (Get-Item Function:\prompt).ScriptBlock

function global:__MighttyWriteOsc {
    param([Parameter(Mandatory = $true)][string] $Value)
    [Console]::Write("`e]$Value`a")
}

function global:prompt {
    $lastCommandSucceeded = $?
    if (
        $script:MighttySemanticIntegrationAvailable -and
        $global:__MighttyCommandPending
    ) {
        $status = if ($lastCommandSucceeded) { 0 } else { 1 }
        __MighttyWriteOsc "133;D;$status"
        $global:__MighttyCommandPending = $false
    }

    if ($PWD.Provider.Name -eq "FileSystem") {
        $uri = [Uri]::new($PWD.ProviderPath).AbsoluteUri
        __MighttyWriteOsc "7;$uri"
    }
    if ($script:MighttySemanticIntegrationAvailable) {
        __MighttyWriteOsc "133;A"
    }

    $promptText = if ($null -ne $script:MighttyOriginalPrompt) {
        & $script:MighttyOriginalPrompt
    } else {
        "PS> "
    }
    if ($script:MighttySemanticIntegrationAvailable) {
        return "$promptText`e]133;B`a"
    }
    return $promptText
}

Import-Module PSReadLine -ErrorAction SilentlyContinue
$psReadLineOptions = Get-PSReadLineOption -ErrorAction SilentlyContinue
if ($null -ne $psReadLineOptions) {
    $script:MighttyOriginalCommandValidationHandler =
        $psReadLineOptions.CommandValidationHandler
    Set-PSReadLineOption -CommandValidationHandler {
        param([System.Management.Automation.Language.CommandAst] $CommandAst)
        if ($null -ne $script:MighttyOriginalCommandValidationHandler) {
            $script:MighttyOriginalCommandValidationHandler.Invoke($CommandAst)
        }
        if (-not $global:__MighttyCommandPending) {
            __MighttyWriteOsc "133;C"
            $global:__MighttyCommandPending = $true
        }
    }
    $script:MighttySemanticIntegrationAvailable = $true
}
