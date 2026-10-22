; Toss - NSIS installer hooks (Tauri 2).
; Wired via tauri.conf.json: bundle.windows.nsis.installerHooks (see installer/README.md).
;
; Adds ONE inbound Windows Defender Firewall rule named "Toss": allow UDP for
; Toss.exe (any local port, so the fixed host port 41820 and random ports both
; work). iroh/QUIC only needs UDP inbound; TCP is not opened.
;
; The rule needs an elevated installer. With installMode "both" the installer
; runs elevated when the user is an administrator (UAC prompt at start; choose
; "for all users" on the studio host). In a non-elevated per-user install netsh
; fails and we ignore it - Toss still works through relays, and the app's
; Settings > Ket noi > "Cho phep qua firewall" button adds the same rule later
; (one UAC prompt). Same rule name everywhere (installer, app, windows-host.ps1)
; so they replace each other instead of piling up.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Delete every rule named "Toss" first (an older program-only rule for any
  ; protocol, or one from a previous install path), then add the UDP rule.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Toss"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Toss" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=UDP enable=yes profile=any'
  Pop $0
  DetailPrint "Toss firewall rule (UDP, inbound): netsh exit code $0"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Toss"'
  Pop $0
  ; Right-click "Tao link Toss" (v0.3): written per user by Toss itself
  ; (src-tauri/src/ctxmenu.rs, WIN_KEYS). Removed for the user running the uninstaller.
  DeleteRegKey HKCU "Software\Classes\Directory\shell\Toss"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\Toss"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
