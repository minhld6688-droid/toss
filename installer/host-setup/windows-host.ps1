<#
.SYNOPSIS
  One-time setup of the unattended Toss host machine (Windows 10/11).

.DESCRIPTION
  1. Disables sleep / hibernate (AC power) so the host stays reachable.
  2. Creates the Scheduled Task "Toss": starts "Toss.exe --hidden" at logon of the
     current user, with highest privileges, restarting it if it crashes.
  3. Removes the per-user "Run" autostart entry named Toss (the task replaces it,
     so Toss is not started twice).
  4. Makes sure an inbound firewall rule for Toss.exe exists.
  5. Prints a checklist of the manual steps (auto-login, power loss, updates).

  Run from an elevated PowerShell:
    powershell -ExecutionPolicy Bypass -File .\windows-host.ps1
  Remove everything it created:
    powershell -ExecutionPolicy Bypass -File .\windows-host.ps1 -Uninstall

  Safe to run again (idempotent). File is intentionally ASCII only.
#>
[CmdletBinding()]
param(
  [string]$ExePath,                                   # auto-detected when omitted
  [string]$UserName = "$env:USERDOMAIN\$env:USERNAME", # account that logs in on the host
  [string]$TaskName = 'Toss',
  [switch]$SkipPower,
  [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'

function Write-Step($m) { Write-Host "[*] $m" -ForegroundColor Cyan }
function Write-Ok($m)   { Write-Host "[+] $m" -ForegroundColor Green }
function Write-Warn2($m){ Write-Host "[!] $m" -ForegroundColor Yellow }

# --- admin check -----------------------------------------------------------
$me = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  Write-Host 'This script must run as Administrator (right-click PowerShell -> Run as administrator).' -ForegroundColor Red
  exit 1
}

# --- uninstall -------------------------------------------------------------
if ($Uninstall) {
  Write-Step "Removing scheduled task '$TaskName'"
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Write-Ok 'Done. Power settings and the firewall rule were left as they are.'
  exit 0
}

# --- locate Toss.exe -------------------------------------------------------
if (-not $ExePath) {
  $candidates = @(
    (Join-Path $env:ProgramFiles 'Toss\Toss.exe'),
    (Join-Path $env:LOCALAPPDATA 'Programs\Toss\Toss.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Toss\Toss.exe')
  )
  $ExePath = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $ExePath -or -not (Test-Path $ExePath)) {
  Write-Host 'Toss.exe not found. Install Toss first, or pass -ExePath "C:\path\to\Toss.exe".' -ForegroundColor Red
  exit 1
}
$ExePath = (Resolve-Path $ExePath).Path
Write-Ok "Using $ExePath"

# --- 1. power: never sleep -------------------------------------------------
if (-not $SkipPower) {
  Write-Step 'Disabling sleep and hibernate on AC power'
  powercfg /change standby-timeout-ac 0   | Out-Null
  powercfg /change hibernate-timeout-ac 0 | Out-Null
  powercfg /change disk-timeout-ac 0      | Out-Null
  # Closing the lid (laptops) must do nothing on AC. 0 = Do nothing.
  powercfg /setacvalueindex SCHEME_CURRENT SUB_BUTTONS LIDACTION 0 2>$null | Out-Null
  powercfg /setactive SCHEME_CURRENT | Out-Null
  Write-Ok 'Sleep/hibernate off on AC (screen may still turn off, that is fine)'
}

# --- 2. scheduled task -----------------------------------------------------
Write-Step "Creating scheduled task '$TaskName' (at logon of $UserName, highest privileges)"
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue

$action    = New-ScheduledTaskAction -Execute $ExePath -Argument '--hidden' -WorkingDirectory (Split-Path $ExePath)
$trigger   = New-ScheduledTaskTrigger -AtLogOn -User $UserName
$principal = New-ScheduledTaskPrincipal -UserId $UserName -LogonType Interactive -RunLevel Highest
$settings  = New-ScheduledTaskSettingsSet `
  -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
  -StartWhenAvailable `
  -MultipleInstances IgnoreNew `
  -ExecutionTimeLimit ([TimeSpan]::Zero) `
  -RestartCount 5 -RestartInterval (New-TimeSpan -Minutes 1)

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
  -Principal $principal -Settings $settings `
  -Description 'Starts Toss hidden at logon so the studio host is always reachable.' | Out-Null
Write-Ok "Task '$TaskName' registered"

# --- 3. drop duplicate autostart ------------------------------------------
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Get-ItemProperty -Path $runKey -Name 'Toss' -ErrorAction SilentlyContinue) {
  Remove-ItemProperty -Path $runKey -Name 'Toss' -ErrorAction SilentlyContinue
  Write-Ok "Removed HKCU Run entry 'Toss' (the scheduled task replaces it)"
}
Write-Warn2 "Keep Toss's own 'start with Windows' setting OFF on this machine: the task already does it."

# --- 4. firewall -----------------------------------------------------------
Write-Step 'Checking inbound firewall rule'
$rule = Get-NetFirewallRule -DisplayName 'Toss' -ErrorAction SilentlyContinue
if (-not $rule) {
  New-NetFirewallRule -DisplayName 'Toss' -Direction Inbound -Action Allow -Program $ExePath -Profile Any | Out-Null
  Write-Ok 'Firewall rule "Toss" created'
} else {
  Write-Ok 'Firewall rule "Toss" already present'
}

# --- 5. start it now -------------------------------------------------------
Write-Step 'Starting the task now'
Start-ScheduledTask -TaskName $TaskName
Start-Sleep -Seconds 3
$running = Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($ExePath)) -ErrorAction SilentlyContinue
if ($running) { Write-Ok 'Toss is running (hidden, see the tray icon)' } else { Write-Warn2 'Toss process not seen yet; check Task Scheduler > Toss > History' }

# --- checklist -------------------------------------------------------------
Write-Host ''
Write-Host '=== Checklist for the studio host ===' -ForegroundColor White
Write-Host ' [ ] Open Toss once -> Host mode ON -> add root folders (RAW, Edit, ...) and the return folder'
Write-Host ' [ ] Auto-login: run  netplwiz  -> untick "Users must enter a user name and password" -> OK'
Write-Host '     (if the checkbox is missing: Settings > Accounts > Sign-in options > turn off "Require Windows Hello sign-in")'
Write-Host ' [ ] BIOS/UEFI: "Restore on AC Power Loss" = Power On (so it comes back after a blackout)'
Write-Host ' [ ] Windows Update: set Active hours; restarts are fine, the task + auto-login bring Toss back'
Write-Host ' [ ] Keep the PC on wired Ethernet if possible'
Write-Host ' [ ] Optional: forward ONE UDP port on the router to this PC for faster direct links (see docs/INSTALL.md)'
Write-Host ' [ ] Reboot once and confirm Toss is back by itself (tray icon, or "Get-ScheduledTask Toss")'
Write-Host ''
