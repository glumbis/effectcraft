//! Opening the OpenFX scan report and the per-user plug-in folder with the operating system's own
//! programs (the engine commands `effect.plugins.openOfxReport` and `effect.plugins.openOfxUserFolder`
//! call [`open_command`]).
//!
//! * `ofx_scan.txt`, in the config folder, is what the app writes at every start-up: the folders it
//!   searched, the effects it registered, why a plug-in did not work. It opens in whatever the system
//!   uses for text files.
//! * `<config folder>/OFX` is the per-user plug-in folder ([`crate::loader::default_search_paths`]);
//!   it is created when it is opened, so there is something to drop a `.ofx.bundle` into.
//!
//! The programs are Explorer (Windows), `open` (macOS) and `xdg-open` (everything else), started
//! detached with the path as their only argument.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The scan report's file name in the config folder (written by the app at every start-up).
pub const SCAN_REPORT: &str = "ofx_scan.txt";
/// The per-user plug-in folder inside the config folder.
pub const USER_FOLDER: &str = "OFX";

/// The path `what` (`report` or `userFolder`) stands for inside the config folder `dir`; the plug-in
/// folder is created if it is not there yet. An error says what is missing.
fn target(what: &str, dir: &Path) -> Result<PathBuf, String> {
    match what {
        "report" => {
            let file = dir.join(SCAN_REPORT);
            if file.is_file() {
                Ok(file)
            } else {
                Err(format!("there is no scan report yet ({} is written when EffectCraft starts, in {})", SCAN_REPORT, dir.display()))
            }
        }
        "userFolder" => {
            let folder = dir.join(USER_FOLDER);
            std::fs::create_dir_all(&folder).map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
            Ok(folder)
        }
        other => Err(format!("unknown thing to open: {other} (report or userFolder)")),
    }
}

/// The program that opens a path with the system's default application.
fn opener() -> &'static str {
    if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Start the system's opener on `path` and let it run on its own.
fn spawn_opener(path: &Path) -> Result<(), String> {
    let program = opener();
    let mut child = Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start {program} to open {}: {e}", path.display()))?;
    // Reap the child when it ends (it may take a while to hand over to the real application); nothing
    // waits for it and its exit status is not an answer (Explorer reports 1 for success).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Engine hook behind `effect.plugins.openOfxReport` (`what` = `report`) and
/// `effect.plugins.openOfxUserFolder` (`what` = `userFolder`): open the scan report or the per-user
/// plug-in folder with the operating system and answer `{path, opened}`.
pub fn open_command(what: &str) -> Result<Value, String> {
    let dir = crate::blocklist::state_dir().ok_or("EffectCraft has no config folder here, so there is no scan report or plug-in folder to open")?;
    let path = target(what, &dir)?;
    spawn_opener(&path)?;
    Ok(json!({"path": path.display().to_string(), "opened": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("effectcraft-ofx-open-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_report_is_the_file_in_the_config_folder_when_it_exists() {
        let dir = scratch("report");
        let e = target("report", &dir).unwrap_err();
        assert!(e.contains("no scan report yet") && e.contains(SCAN_REPORT), "{e}");
        std::fs::write(dir.join(SCAN_REPORT), "report").unwrap();
        assert_eq!(target("report", &dir), Ok(dir.join("ofx_scan.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plugin_folder_is_made_when_it_is_opened() {
        let dir = scratch("folder");
        assert!(!dir.join("OFX").exists());
        assert_eq!(target("userFolder", &dir), Ok(dir.join("OFX")));
        assert!(dir.join("OFX").is_dir());
        // Opening it again, with a plug-in in it, changes nothing.
        std::fs::create_dir_all(dir.join("OFX").join("Mine.ofx.bundle")).unwrap();
        assert_eq!(target("userFolder", &dir), Ok(dir.join("OFX")));
        assert!(dir.join("OFX").join("Mine.ofx.bundle").is_dir());
        // A config folder that cannot hold it (a file is in the way) is an error, not a panic.
        let file = dir.join("not-a-folder");
        std::fs::write(&file, "x").unwrap();
        assert!(target("userFolder", &file).unwrap_err().contains("cannot create"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anything_else_is_not_opened() {
        let dir = scratch("other");
        let e = target("../etc/passwd", &dir).unwrap_err();
        assert!(e.contains("unknown thing to open"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without a config folder (tools, tests) there is nothing to open.
    #[test]
    fn no_config_folder_is_an_error() {
        if crate::blocklist::state_dir().is_none() {
            assert!(open_command("report").unwrap_err().contains("no config folder"));
        }
    }
}
