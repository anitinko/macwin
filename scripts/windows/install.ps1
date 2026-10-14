<#
.SYNOPSIS
Installs MacWin for the current user.

.DESCRIPTION
Copies macwin.exe and macwin-tray.exe to %LOCALAPPDATA%\Programs\MacWin, adds
that folder to the user's PATH, starts the tray app at login, allows MacWin
through Windows Firewall on private networks (one administrator prompt) and
starts the tray app. Run it from the unpacked folder:

    powershell -ExecutionPolicy Bypass -File .\install.ps1

Installing again updates MacWin; the settings and the pairing stay.

.PARAMETER NoFirewall
Skips the firewall rules (Windows then asks when MacWin first listens).

.PARAMETER NoAutostart
Does not start MacWin at login.
#>
[CmdletBinding()]
param(
    [switch]$NoFirewall,
    [switch]$NoAutostart
)

$ErrorActionPreference = 'Stop'
$Source = $PSScriptRoot
$Dest = Join-Path $env:LOCALAPPDATA 'Programs\MacWin'
$Files = @('macwin.exe', 'macwin-tray.exe', 'uninstall.ps1', 'README.txt', 'THIRD_PARTY_NOTICES.txt')

foreach ($f in $Files) {
    if (-not (Test-Path -LiteralPath (Join-Path $Source $f))) {
        throw "$f is missing next to install.ps1"
    }
}

# A running copy keeps its files locked.
Get-Process -Name 'macwin-tray', 'macwin' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($Dest, [StringComparison]::OrdinalIgnoreCase) } |
    Stop-Process -Force
Start-Sleep -Milliseconds 500

New-Item -ItemType Directory -Force -Path $Dest | Out-Null
foreach ($f in $Files) {
    Copy-Item -Force -LiteralPath (Join-Path $Source $f) -Destination $Dest
}
# Files from a downloaded zip carry the "downloaded from the Internet" mark,
# which makes Windows warn at every start; the user chose to install them.
Get-ChildItem -LiteralPath $Dest | Unblock-File

# `macwin` in new terminals. The registry value is edited directly so that
# entries such as %USERPROFILE%\bin stay unexpanded (REG_EXPAND_SZ).
$envKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
$raw = [string]$envKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
$parts = @($raw -split ';' | Where-Object { $_ -ne '' })
if ($parts -notcontains $Dest) {
    $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    if ($null -ne $envKey.GetValue('Path')) {
        $kind = $envKey.GetValueKind('Path')
    }
    $envKey.SetValue('Path', (($parts + $Dest) -join ';'), $kind)
    # Removing a variable that does not exist only broadcasts the
    # "environment changed" message, so new terminals see the PATH.
    [Environment]::SetEnvironmentVariable('MACWIN_INSTALL', $null, 'User')
}
$envKey.Close()

if (-not $NoAutostart) {
    & (Join-Path $Dest 'macwin.exe') autostart on
    if ($LASTEXITCODE -ne 0) {
        Write-Warning 'MacWin could not set up the start at login.'
    }
}

if (-not $NoFirewall) {
    $tray = (Join-Path $Dest 'macwin-tray.exe').Replace("'", "''")
    $cli = (Join-Path $Dest 'macwin.exe').Replace("'", "''")
    # Earlier answers to the firewall prompt become rules of their own (a
    # block rule would win over an allow rule): replace them.
    $rules = @"
Remove-NetFirewallRule -Group 'MacWin' -ErrorAction SilentlyContinue
foreach (`$p in @('$tray', '$cli')) {
    Get-NetFirewallApplicationFilter -Program `$p -ErrorAction SilentlyContinue | Get-NetFirewallRule | Remove-NetFirewallRule
}
New-NetFirewallRule -Group 'MacWin' -DisplayName 'MacWin' -Direction Inbound -Action Allow -Protocol UDP -Profile Private,Domain -Program '$tray' | Out-Null
New-NetFirewallRule -Group 'MacWin' -DisplayName 'MacWin (command line)' -Direction Inbound -Action Allow -Protocol UDP -Profile Private,Domain -Program '$cli' | Out-Null
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($rules))
    try {
        $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden `
            -ArgumentList "-NoProfile -ExecutionPolicy Bypass -EncodedCommand $encoded"
        if ($p.ExitCode -ne 0) {
            Write-Warning 'The firewall rules were not added.'
        }
    } catch {
        Write-Warning 'The firewall rules were not added (no administrator rights); Windows asks when MacWin first listens.'
    }
}

Start-Process -FilePath (Join-Path $Dest 'macwin-tray.exe')
Write-Host "MacWin is installed in $Dest; its icon is in the notification area."
Write-Host 'Pair it from the icon menu ("Pair with the other computer..."), or run: macwin pair'
