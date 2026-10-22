<#
.SYNOPSIS
  One-time setup of the unattended Toss host machine (Windows 10/11).

.DESCRIPTION
  1. Disables sleep / hibernate (AC power) so the host stays reachable.
  2. Creates the Scheduled Task "Toss": starts "Toss.exe --hidden" at logon of the
     current user, with highest privileges, restarting it if it crashes.
  3. Removes the per-user "Run" autostart entry named Toss (the task replaces it,
     so Toss is not started twice).
  4. Makes sure ONE inbound UDP allow rule named "Toss" exists for Toss.exe
     (an older program-only rule, any protocol, is replaced).
  5. Starts the task, then checks that UDP port 41820 (the fixed host port) is
     bound by Toss.exe, and - when toss-cli.exe sits next to Toss.exe or is in
     PATH - prints the direct-connection likelihood and port-mapping lines of
     "toss-cli diag". Both checks only warn; they never fail the script.
  6. Prints a checklist of the manual steps (auto-login, power loss, updates,
     router UPnP / port forward of UDP 41820).

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
  [switch]$SkipDiag,                                   # skip the toss-cli diag run
  [int]$HostPort = 41820,                              # Toss's fixed host UDP port
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
# ONE inbound UDP allow rule named "Toss" for Toss.exe (iroh/QUIC is UDP only).
# The installer (hooks.nsh), the app (Settings > Ket noi) and this script all use
# the same name, so they replace each other. Older program-only rules (any
# protocol), rules for another install path, and the rules Windows creates by
# itself when its "allow this app?" prompt is answered are removed first; a stale
# Block rule would otherwise win over our Allow rule.
Write-Step 'Checking inbound firewall rule (UDP, Toss.exe)'
$existing = @(Get-NetFirewallRule -DisplayName 'Toss' -ErrorAction SilentlyContinue)
$ruleOk = $false
if ($existing.Count -eq 1) {
  $r = $existing[0]
  $app  = $r | Get-NetFirewallApplicationFilter
  $port = $r | Get-NetFirewallPortFilter
  $ruleOk = ("$($r.Enabled)" -eq 'True') -and ("$($r.Direction)" -eq 'Inbound') -and ("$($r.Action)" -eq 'Allow') -and
            ("$($port.Protocol)" -eq 'UDP') -and ("$($app.Program)" -ieq $ExePath)
}
if ($ruleOk) {
  Write-Ok 'Firewall rule "Toss" (inbound UDP, Toss.exe) already present'
} else {
  if ($existing.Count -gt 0) {
    $existing | Remove-NetFirewallRule
    Write-Ok "Removed $($existing.Count) old firewall rule(s) named 'Toss'"
  }
  New-NetFirewallRule -DisplayName 'Toss' -Direction Inbound -Action Allow -Protocol UDP -Program $ExePath -Profile Any | Out-Null
  Write-Ok 'Firewall rule "Toss" created (inbound UDP, Toss.exe, all profiles)'
}

# --- 5. start it now -------------------------------------------------------
Write-Step 'Starting the task now'
Start-ScheduledTask -TaskName $TaskName
Start-Sleep -Seconds 3
$running = Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($ExePath)) -ErrorAction SilentlyContinue
if ($running) { Write-Ok 'Toss is running (hidden, see the tray icon)' } else { Write-Warn2 'Toss process not seen yet; check Task Scheduler > Toss > History' }

# --- 6. is the fixed host port bound by Toss? ---------------------------------
# Toss only listens on UDP $HostPort once Host mode is ON (and falls back to a
# random port when $HostPort is taken). Warn only: the first run has no host mode yet.
Write-Step "Checking that UDP $HostPort is bound by Toss"
$exeBase = [IO.Path]::GetFileNameWithoutExtension($ExePath)
$bound = $null
for ($i = 0; $i -lt 15; $i++) {
  $bound = Get-NetUDPEndpoint -LocalPort $HostPort -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($bound) { break }
  Start-Sleep -Seconds 2
}
if ($bound) {
  $owner = Get-Process -Id $bound.OwningProcess -ErrorAction SilentlyContinue
  if ($owner -and $owner.ProcessName -ieq $exeBase) {
    Write-Ok "UDP $HostPort is bound by $($owner.ProcessName) (pid $($owner.Id))"
  } else {
    $who = if ($owner) { "$($owner.ProcessName) (pid $($owner.Id))" } else { "pid $($bound.OwningProcess)" }
    Write-Warn2 "UDP $HostPort is used by $who, not Toss. Toss will pick a random port instead, so a router forward of $HostPort will not help. Close that program or change Toss's UDP port."
  }
} else {
  Write-Warn2 "Nothing is bound to UDP $HostPort yet. Open Toss and switch Host mode ON (it listens only in host mode), then check again: Get-NetUDPEndpoint -LocalPort $HostPort"
}

# --- 7. connection diagnostics via toss-cli (optional) ---------------------------
# Runs `toss-cli diag` with a THROWAWAY identity (temp data dir): the running app owns
# the real key and must not be disturbed. It reports this network's NAT, UPnP/port
# mapping and direct-connection likelihood; it does not test port $HostPort itself.
function Find-TossCli {
  $beside = Join-Path (Split-Path $ExePath) 'toss-cli.exe'
  if (Test-Path $beside) { return $beside }
  $cmd = Get-Command 'toss-cli.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($cmd) { return $cmd.Source }
  return $null
}
$cli = Find-TossCli
if ($SkipDiag) {
  Write-Host '[-] Skipping toss-cli diag (-SkipDiag)'
} elseif (-not $cli) {
  Write-Host '[-] toss-cli.exe not found next to Toss.exe or in PATH: skipping diagnostics (see Settings > Ket noi in the app)'
} else {
  Write-Step "Running toss-cli diag ($cli)"
  $tmp = Join-Path $env:TEMP ('toss-diag-' + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Force $tmp | Out-Null
  try {
    $outFile = Join-Path $tmp 'out.json'
    $errFile = Join-Path $tmp 'err.txt'
    $cliArgs = @('--data-dir', ('"{0}"' -f $tmp), '--name', 'toss-diag', 'diag')
    $p = Start-Process -FilePath $cli -ArgumentList $cliArgs -RedirectStandardOutput $outFile -RedirectStandardError $errFile -PassThru -NoNewWindow
    $null = $p.Handle   # keep the handle so ExitCode is readable
    if (-not $p.WaitForExit(60000)) {
      Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
      Write-Warn2 'toss-cli diag did not finish within 60 s; skipped'
    } elseif ($p.ExitCode -ne 0) {
      Write-Warn2 "toss-cli diag exited with code $($p.ExitCode) (older toss-cli without 'diag'?)"
      Get-Content $errFile -Tail 5 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "    $_" }
    } else {
      $d = Get-Content -Raw $outFile | ConvertFrom-Json
      $pm = $d.portmap
      $pmLine = "status=$($pm.status)"
      if ($pm.external_addr) { $pmLine += " external=$($pm.external_addr)" }
      if ($pm.protocol)      { $pmLine += " protocol=$($pm.protocol)" }
      Write-Host "    direct likelihood : $($d.direct_likelihood)   (high = editors connect Direct; low = expect Relay)"
      Write-Host "    port mapping      : $pmLine"
      Write-Host "    UDP works         : $($d.nat.udp)   NAT mapping varies by destination: $($d.nat.mapping_varies_by_dest)"
      Write-Host "    relay             : $($d.relay_url)   connected: $($d.relay_connected)"
      foreach ($reason in @($d.reasons)) { if ($reason) { Write-Host "    - $reason" } }
      if ("$($d.direct_likelihood)" -eq 'unknown') { Write-Warn2 'Net report was not ready yet; run the script again in a minute.' }
    }
  } catch {
    Write-Warn2 "toss-cli diag failed: $($_.Exception.Message)"
  } finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }
}

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
