# installer/

Platform hooks consumed by the Tauri bundler, plus the studio host setup script.

| Path | Used by |
| --- | --- |
| `windows/hooks.nsh` | NSIS installer (firewall rule on install, removed on uninstall) |
| `macos/entitlements.plist` | macOS code signing (hardened runtime entitlements) |
| `host-setup/windows-host.ps1` | run once on the unattended studio host (see `host-setup/README.md`) |

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
        "installerHooks": "../../installer/windows/hooks.nsh",
        "installMode": "both"            // lets the host install per-machine (UAC) so the firewall rule applies
      }
    },
    "macOS": {
      "entitlements": "../../installer/macos/entitlements.plist",
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
- `installMode: "both"` shows the "for me only / for all users" choice. Editors can pick "for me only"
  (no UAC, no firewall rule, relays still work). The host machine should pick "for all users".
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
