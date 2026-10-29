//! macOS Finder Quick Action "Gửi bằng Toss": an Automator "Run Shell Script"
//! service bundle at `~/Library/Services/Gửi bằng Toss.workflow` that runs
//! `open -n -a Toss --args --share "<folder>"` for every selected folder.
//!
//! `-n` starts a short-lived second process even when Toss is already running
//! (plain `open -a` would only activate the running app and drop `--args`); that
//! process hands its argv to the running instance through
//! tauri-plugin-single-instance and exits, like a `toss://` link on Windows.
//!
//! The file builders and install/remove are plain functions over a directory so
//! they are unit-tested on every OS; only `ctxmenu.rs` calls them, on macOS.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::ctxmenu::{LABEL, LEGACY_LABELS, SHARE_FLAG};

/// Shell script run by the Quick Action (input passed "as arguments").
pub fn shell_command() -> String {
    format!(r#"for f in "$@"; do open -n -a Toss --args {SHARE_FLAG} "$f"; done"#)
}

/// `<LABEL>.workflow`.
pub fn bundle_name() -> String {
    format!("{LABEL}.workflow")
}

/// `~/Library/Services`.
pub fn services_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library").join("Services"))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const PLIST_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
"#;

/// `Contents/Info.plist`: registers the service for folders, in Finder only.
pub fn info_plist() -> String {
    let name = xml_escape(LABEL);
    format!(
        r#"{PLIST_HEAD}<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en_US</string>
	<key>CFBundleIdentifier</key>
	<string>com.minhfatz.toss.quickaction</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSBackgroundColorName</key>
			<string>background</string>
			<key>NSIconName</key>
			<string>NSActionTemplate</string>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>{name}</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendTypes</key>
			<array>
				<string>NSFilenamesPboardType</string>
			</array>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.folder</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#
    )
}

/// `Contents/document.wflow`: one "Run Shell Script" action (/bin/bash, input as
/// arguments), workflow type = Quick Action/service, input = folders in Finder.
pub fn document_wflow() -> String {
    let cmd = xml_escape(&shell_command());
    format!(
        r#"{PLIST_HEAD}<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{cmd}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/bash</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>6E3B4C1A-2F7D-4E2B-9C1D-0A1B2C3D4E01</string>
				<key>Keywords</key>
				<array>
					<string>Shell</string>
					<string>Script</string>
					<string>Command</string>
					<string>Run</string>
					<string>Unix</string>
				</array>
				<key>OutputUUID</key>
				<string>6E3B4C1A-2F7D-4E2B-9C1D-0A1B2C3D4E02</string>
				<key>UUID</key>
				<string>6E3B4C1A-2F7D-4E2B-9C1D-0A1B2C3D4E03</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>arguments</key>
				<dict>
					<key>0</key>
					<dict>
						<key>default value</key>
						<integer>0</integer>
						<key>name</key>
						<string>inputMethod</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>0</string>
					</dict>
					<key>1</key>
					<dict>
						<key>default value</key>
						<string></string>
						<key>name</key>
						<string>source</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>1</string>
					</dict>
					<key>2</key>
					<dict>
						<key>default value</key>
						<false/>
						<key>name</key>
						<string>CheckedForUserDefaultShell</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>2</string>
					</dict>
					<key>3</key>
					<dict>
						<key>default value</key>
						<string></string>
						<key>name</key>
						<string>COMMAND_STRING</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>3</string>
					</dict>
					<key>4</key>
					<dict>
						<key>default value</key>
						<string>/bin/sh</string>
						<key>name</key>
						<string>shell</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>4</string>
					</dict>
				</dict>
				<key>isViewVisible</key>
				<integer>1</integer>
				<key>location</key>
				<string>309.000000:253.000000</string>
				<key>nibPath</key>
				<string>/System/Library/Automator/Run Shell Script.action/Contents/Resources/Base.lproj/main.nib</string>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleID</key>
		<string>com.apple.finder</string>
		<key>applicationBundleIDsByPath</key>
		<dict>
			<key>/System/Library/CoreServices/Finder.app</key>
			<string>com.apple.finder</string>
		</dict>
		<key>applicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>applicationPaths</key>
		<array>
			<string>/System/Library/CoreServices/Finder.app</string>
		</array>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject.folder</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<integer>0</integer>
		<key>serviceApplicationBundleID</key>
		<string>com.apple.finder</string>
		<key>serviceApplicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject.folder</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<integer>0</integer>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<integer>0</integer>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#
    )
}

/// The bundle's files, relative to the `.workflow` folder.
fn files() -> [(&'static str, String); 2] {
    [
        ("Contents/Info.plist", info_plist()),
        ("Contents/document.wflow", document_wflow()),
    ]
}

/// Writes the bundle into `services` (atomically: built in a temp folder next to
/// it, then renamed over the old one). Returns false if it was already up to date.
pub fn install(services: &Path) -> Result<bool> {
    let legacy = remove_legacy(services);
    let target = services.join(bundle_name());
    let files = files();
    let current = files
        .iter()
        .all(|(rel, body)| fs::read(target.join(rel)).ok().as_deref() == Some(body.as_bytes()));
    if current {
        return Ok(legacy);
    }
    fs::create_dir_all(services)
        .with_context(|| format!("không tạo được {}", services.display()))?;
    let tmp = services.join(format!(".{}.tmp-{}", bundle_name(), std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let built = (|| -> Result<()> {
        for (rel, body) in &files {
            let p = tmp.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&p, body)?;
        }
        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
        fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if let Err(e) = built {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e.context(format!("không ghi được {}", target.display())));
    }
    Ok(true)
}

/// Deletes bundles left by older versions under their old names. True if any was removed.
fn remove_legacy(services: &Path) -> bool {
    let mut any = false;
    for old in LEGACY_LABELS {
        let p = services.join(format!("{old}.workflow"));
        if p.exists() && fs::remove_dir_all(&p).is_ok() {
            any = true;
        }
    }
    any
}

/// Deletes the bundle (and old-named ones). Returns false if there was none.
pub fn remove(services: &Path) -> Result<bool> {
    let legacy = remove_legacy(services);
    let target = services.join(bundle_name());
    if !target.exists() {
        return Ok(legacy);
    }
    fs::remove_dir_all(&target).with_context(|| format!("không xoá được {}", target.display()))?;
    Ok(true)
}

/// Asks macOS to re-read the Services menu (best effort; it also notices on its own).
pub fn refresh() {
    let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
        .arg("-update")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> plist::Value {
        plist::Value::from_reader_xml(xml.as_bytes()).expect("valid plist")
    }

    #[test]
    fn info_plist_declares_a_finder_folder_service() {
        let v = parse(&info_plist());
        let svc = &v.as_dictionary().unwrap()["NSServices"].as_array().unwrap()[0];
        let svc = svc.as_dictionary().unwrap();
        assert_eq!(
            svc["NSMenuItem"].as_dictionary().unwrap()["default"].as_string(),
            Some("Gửi bằng Toss")
        );
        assert_eq!(svc["NSMessage"].as_string(), Some("runWorkflowAsService"));
        assert_eq!(
            svc["NSRequiredContext"].as_dictionary().unwrap()["NSApplicationIdentifier"]
                .as_string(),
            Some("com.apple.finder")
        );
        assert_eq!(
            svc["NSSendFileTypes"].as_array().unwrap()[0].as_string(),
            Some("public.folder")
        );
        assert!(svc.contains_key("NSSendTypes"));
    }

    #[test]
    fn wflow_runs_the_share_command() {
        let v = parse(&document_wflow());
        let d = v.as_dictionary().unwrap();
        let action = d["actions"].as_array().unwrap()[0].as_dictionary().unwrap()["action"]
            .as_dictionary()
            .unwrap();
        let params = action["ActionParameters"].as_dictionary().unwrap();
        let cmd = params["COMMAND_STRING"].as_string().unwrap();
        assert_eq!(cmd, shell_command());
        assert_eq!(
            cmd,
            r#"for f in "$@"; do open -n -a Toss --args --share "$f"; done"#
        );
        assert_eq!(params["inputMethod"].as_signed_integer(), Some(1));
        assert_eq!(params["shell"].as_string(), Some("/bin/bash"));
        let meta = d["workflowMetaData"].as_dictionary().unwrap();
        assert_eq!(
            meta["workflowTypeIdentifier"].as_string(),
            Some("com.apple.Automator.servicesMenu")
        );
        assert_eq!(
            meta["serviceInputTypeIdentifier"].as_string(),
            Some("com.apple.Automator.fileSystemObject.folder")
        );
        assert_eq!(
            meta["serviceApplicationBundleID"].as_string(),
            Some("com.apple.finder")
        );
    }

    #[test]
    fn install_is_atomic_idempotent_and_removable() {
        let dir = std::env::temp_dir().join(format!("toss-services-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let bundle = dir.join("Gửi bằng Toss.workflow");
        // A Quick Action left by 0.5 under its old name is cleaned up.
        let old = dir.join("Tạo link Toss.workflow");
        fs::create_dir_all(old.join("Contents")).unwrap();
        assert!(install(&dir).unwrap(), "first install writes");
        assert!(!old.exists(), "old-named bundle removed");
        assert!(bundle.join("Contents/Info.plist").is_file());
        assert!(bundle.join("Contents/document.wflow").is_file());
        assert!(!install(&dir).unwrap(), "second install is a no-op");
        // A stale/edited bundle is replaced as a whole.
        fs::write(bundle.join("Contents/document.wflow"), "old").unwrap();
        fs::write(bundle.join("Contents/stray"), "x").unwrap();
        assert!(install(&dir).unwrap());
        assert!(!bundle.join("Contents/stray").exists());
        assert_eq!(
            fs::read_to_string(bundle.join("Contents/document.wflow")).unwrap(),
            document_wflow()
        );
        // No temp folder left behind.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
        assert!(remove(&dir).unwrap());
        assert!(!bundle.exists());
        assert!(!remove(&dir).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }
}
