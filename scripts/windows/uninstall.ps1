<#
.SYNOPSIS
Removes MacWin for the current user.

.DESCRIPTION
Stops MacWin and removes the start at login, the firewall rules (one
administrator prompt), the PATH entry and %LOCALAPPDATA%\Programs\MacWin.
The settings and the identity (the pairing) stay unless -Purge is given.

    powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\MacWin\uninstall.ps1"

.PARAMETER Purge
Also deletes the settings, the identity and received files.
#>
[CmdletBinding()]
param(
    [switch]$Purge
)

$ErrorActionPreference = 'Stop'
$Dest = Join-Path $env:LOCALAPPDATA 'Programs\MacWin'
Set-Location -LiteralPath $env:TEMP

$running = @(Get-Process -Name 'macwin-tray', 'macwin' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith("$Dest\", [StringComparison]::OrdinalIgnoreCase) })
if ($running.Count -gt 0) {
    $running | Stop-Process -Force
    $running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
}

Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'MacWin' -ErrorAction SilentlyContinue

$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(
    "Remove-NetFirewallRule -Group 'MacWin' -ErrorAction SilentlyContinue"))
try {
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -WindowStyle Hidden `
        -ArgumentList "-NoProfile -ExecutionPolicy Bypass -EncodedCommand $encoded"
} catch {
    Write-Warning 'The firewall rules stay (no administrator rights); remove the "MacWin" rules in Windows Defender Firewall.'
}

# Edits the registry value directly, keeping its type (see install.ps1).
$envKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
if ($null -ne $envKey.GetValue('Path')) {
    $raw = [string]$envKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $all = @($raw -split ';' | Where-Object { $_ -ne '' })
    $parts = @($all | Where-Object { $_ -ne $Dest })
    if ($parts.Count -ne $all.Count) {
        $envKey.SetValue('Path', ($parts -join ';'), $envKey.GetValueKind('Path'))
        [Environment]::SetEnvironmentVariable('MACWIN_INSTALL', $null, 'User')
    }
}
$envKey.Close()

if (Test-Path -LiteralPath $Dest) {
    Remove-Item -Recurse -Force -LiteralPath $Dest
}
if ($Purge) {
    foreach ($dir in @((Join-Path $env:APPDATA 'macwin'), (Join-Path $env:LOCALAPPDATA 'macwin'))) {
        if (Test-Path -LiteralPath $dir) {
            Remove-Item -Recurse -Force -LiteralPath $dir
        }
    }
}
Write-Host 'MacWin is removed.'
