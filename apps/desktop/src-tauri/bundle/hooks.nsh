; Toss - NSIS installer hooks (Tauri 2).
; Wired via tauri.conf.json: bundle.windows.nsis.installerHooks (see installer/README.md).
;
; Adds an inbound Windows Defender Firewall rule for Toss.exe so a host machine
; accepts direct (non-relay) connections from editors. Needs an elevated
; installer (installMode "both" or "perMachine"); in a per-user, non-elevated
; install netsh fails and we ignore it - Toss still works through iroh relays.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Remove a stale rule from an older install path first, then add the current one.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Toss"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Toss" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" enable=yes profile=any'
  Pop $0
  DetailPrint "Toss firewall rule: netsh exit code $0"
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
