# installer/

Platform hooks consumed by the Tauri bundler. (The 0.4–0.6 studio host script `host-setup/windows-host.ps1`
was removed in 0.8: autostart is on for every machine, see `docs/INSTALL.md` → "Máy luôn bật".)

| Path | Used by |
| --- | --- |
| `apps/desktop/src-tauri/bundle/hooks.nsh` | NSIS installer: inbound firewall rules "Toss" (**UDP**, any port) and "Toss LocalSend" (**TCP + UDP 53317–53327**) for `Toss.exe` on install; on uninstall removes them, the right-click verbs and the *Send to → Toss* shortcut |
| `apps/desktop/src-tauri/bundle/entitlements.plist` | macOS code signing (hardened runtime entitlements) |
| `../hub/relay/` | deploy kit for an own relay (VPS), see its README |

## tauri.conf.json wiring (coordinator / desktop agent)

Paths are relative to `apps/desktop/src-tauri/` (where `tauri.conf.json` lives).

```jsonc
{
  "productName": "Toss",
  "mainBinaryName": "Toss",          // hooks.nsh uses ${MAINBINARYNAME}.exe -> Toss.exe
  "bundle": {
    "active": true,
    "targets": ["nsis", "dmg", "app"],   // no MSI: the release workflow looks for *-setup.exe and *.dmg
    "windows": {
      "nsis": {
        "installerHooks": "bundle/hooks.nsh",
        "installMode": "both"            // "currentUser" | "perMachine" | "both" (Tauri 2 schema); both = user picks at install time
      }
    },
    "macOS": {
      "entitlements": "bundle/entitlements.plist",
      "minimumSystemVersion": "11.0"
    }
  },
  "plugins": {
    "deep-link": { "desktop": { "schemes": ["toss"] } }   // registers toss://t/<ticket> on both OSes
  }
}
```

Notes:
- The release workflow expects exactly one `bundle/dmg/*.dmg` (macOS universal) and one
  `bundle/nsis/*-setup.exe` (Windows). It republishes them as `Toss.dmg` and `Toss-setup.exe`,
  the names the link page (`web/index.html`, constant `TOSS.assets`) downloads from
  `releases/latest/download/`.
- `installMode: "both"` shows the "for me only / for all users" choice. Tauri's `both` mode starts the
  installer with the highest available rights: an **administrator account gets one UAC prompt at start even
  for "for me only"**; a standard (non-admin) account gets no prompt and can only install for itself.
  The host machine should pick "for all users" (installs to `Program Files`, registry under `HKLM`).
- Firewall rules: `hooks.nsh` runs `netsh advfirewall firewall delete rule name="Toss"` then adds
  `name="Toss" dir=in action=allow program="$INSTDIR\Toss.exe" protocol=UDP profile=any` (iroh/QUIC, any local
  port, so the fixed port 41820 and random ports are covered). v0.8 adds `name="Toss LocalSend"` twice:
  `protocol=TCP localport=53317-53327` (LocalSend HTTPS server; 53318–53327 are its fallback ports) and the
  same for UDP (LocalSend multicast discovery).
  The rule is added whenever the installer is elevated (including an admin's "for me only" install). In a
  non-elevated install `netsh` fails, the hook ignores it, the install still succeeds silently (`/S` works), and
  the app's **"Cho phep qua firewall"** button (shell `add_firewall_rule`, one UAC prompt) adds the same rules
  later. Both use the same rule names, so they replace each other. Uninstall deletes the rules.
- Silent install switches (NSIS, Tauri template): `Toss-setup.exe /S` (default mode), `/S /ALLUSERS`
  (per machine), `/S /CURRENTUSER`, `/D=C:\Path` (install dir, must be last).
- The app accepts `--hidden` (start with the window hidden in the tray); the autostart entry (forced on once by
  the 0.8 update, default for new installs) passes it.
- `--send <items…>` (right-click on files, *Send to → Toss*, macOS Quick Action) and `--share <path>` (folder
  verbs) are forwarded to the running app by the single-instance plugin, which opens "Gửi tới ai?".
- Run the app single-instance (`tauri-plugin-single-instance` with the `deep-link` feature) so a
  `toss://` click while Toss is already running forwards the URL instead of starting a second copy.

## Signing and notarization (optional)

Builds are unsigned unless these repository secrets exist; `release.yml` exports them into the job
only when `APPLE_CERTIFICATE` is non-empty. Names are the standard Tauri / tauri-action ones:

| Secret | Meaning |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the "Developer ID Application" `.p12` (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | password of that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_PASSWORD` | app-specific password for that Apple ID |
| `APPLE_TEAM_ID` | 10-character team id |

Unsigned consequences: macOS Gatekeeper requires right-click -> Open on first launch; Windows
SmartScreen shows "More info -> Run anyway". `docs/INSTALL.md` documents both for editors.
Windows code signing is not wired (needs a paid certificate); add it later through
`bundle.windows.signCommand` or Azure Trusted Signing.
