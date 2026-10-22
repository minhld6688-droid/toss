# installer/

Platform hooks consumed by the Tauri bundler, plus the studio host setup script.

| Path | Used by |
| --- | --- |
| `apps/desktop/src-tauri/bundle/hooks.nsh` | NSIS installer (inbound **UDP** firewall rule "Toss" for `Toss.exe` on install, removed on uninstall) |
| `apps/desktop/src-tauri/bundle/entitlements.plist` | macOS code signing (hardened runtime entitlements) |
| `host-setup/windows-host.ps1` | run once on the unattended studio host (see `host-setup/README.md`) |
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
- Firewall rule: `hooks.nsh` runs `netsh advfirewall firewall delete rule name="Toss"` then adds
  `name="Toss" dir=in action=allow program="$INSTDIR\Toss.exe" protocol=UDP profile=any`. Only UDP is opened
  (iroh/QUIC); it works for any local port, so the fixed host port 41820 and random ports are both covered.
  The rule is added whenever the installer is elevated (including an admin's "for me only" install). In a
  non-elevated install `netsh` fails, the hook ignores it, the install still succeeds silently (`/S` works), and
  the app's **Settings -> Ket noi -> "Cho phep qua firewall"** button (shell `add_firewall_rule`, one UAC prompt)
  or `host-setup/windows-host.ps1` adds the same rule later. All three use the same rule name, so they replace
  each other. Uninstall deletes the rule.
- Silent install switches (NSIS, Tauri template): `Toss-setup.exe /S` (default mode), `/S /ALLUSERS`
  (per machine), `/S /CURRENTUSER`, `/D=C:\Path` (install dir, must be last).
- The app must accept `--hidden` (start with the window hidden in the tray); the host script relies on it.
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
