; Toss - NSIS installer hooks (Tauri 2).
; Wired via tauri.conf.json: bundle.windows.nsis.installerHooks (see installer/README.md).
;
; Inbound Windows Defender Firewall rules for Toss.exe:
;   "Toss"           UDP, any local port (iroh/QUIC: random ports and the fixed 41820).
;   "Toss LocalSend" TCP + UDP 53317-53327 (v0.8: LocalSend server + discovery; the
;                    bridge falls back from 53317 to 53318..53327 when the port is busy).
;
; The rules need an elevated installer. With installMode "both" the installer
; runs elevated when the user is an administrator (UAC prompt at start). In a
; non-elevated per-user install netsh fails and we ignore it - Toss still works
; through relays, and the app's "Cho phep qua firewall" button adds the same
; rules later (one UAC prompt, src-tauri/src/firewall.rs). Same rule names
; everywhere so they replace each other instead of piling up.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Delete every rule with our names first (an older program-only rule for any
  ; protocol, or one from a previous install path), then add the rules.
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Toss"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Toss" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=UDP enable=yes profile=any'
  Pop $0
  DetailPrint "Toss firewall rule (UDP, inbound): netsh exit code $0"
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Toss LocalSend"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Toss LocalSend" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=TCP localport=53317-53327 enable=yes profile=any'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Toss LocalSend" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" protocol=UDP localport=53317-53327 enable=yes profile=any'
  Pop $0
  DetailPrint "Toss LocalSend firewall rules (TCP+UDP 53317-53327): netsh exit code $0"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Toss"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Toss LocalSend"'
  Pop $0
  ; Right-click "Gui bang Toss" (folders, folder background, v0.8 files): written per
  ; user by Toss itself (src-tauri/src/ctxmenu.rs, WIN_KEYS). Removed for the user
  ; running the uninstaller.
  DeleteRegKey HKCU "Software\Classes\Directory\shell\Toss"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\Toss"
  DeleteRegKey HKCU "Software\Classes\*\shell\Toss"
  ; v0.8 "Send to -> Toss" shortcut (per user, %APPDATA%; $APPDATA may point to the
  ; all-users folder in a per-machine uninstall, so read the variable).
  ReadEnvStr $1 APPDATA
  Delete "$1\Microsoft\Windows\SendTo\Toss.lnk"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
