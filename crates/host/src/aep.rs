//! After Effects project import (`file.importAep`, the `aep` feature): runs the external converter
//! `tools/aep-import/aep2effectcraft.py`, which reads the `.aep` / `.aepx` with py-aep (MIT, a
//! Python package) and writes the After Effects-style script that rebuilds the project. Needs
//! Python 3 with py-aep installed (`py -m pip install py-aep`).
//!
//! Python: `EFFECTCRAFT_PYTHON`, else `py -3` (the Windows launcher), `python3`, `python`. The
//! converter: `EFFECTCRAFT_AEP_SCRIPT`, else `aep-import/aep2effectcraft.py` next to the
//! executable (release zips), else the repository's `tools/aep-import` (development builds).

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPT: &str = "aep2effectcraft.py";

/// The [`effectcraft_engine::AepConverter`]: the generated script's code, or why there is none.
pub fn convert(path: &str) -> Result<String, String> {
    let script = find_script().ok_or_else(|| {
        format!("the converter {SCRIPT} was not found (looked next to the app in aep-import/ and in tools/aep-import/; set EFFECTCRAFT_AEP_SCRIPT)")
    })?;
    let out = std::env::temp_dir().join(format!("effectcraft-aep-{}.jsx", std::process::id()));
    let mut tried = Vec::new();
    for (exe, pre) in pythons() {
        let mut cmd = Command::new(&exe);
        cmd.args(&pre).arg(&script).arg(path).arg("-o").arg(&out);
        hide_console(&mut cmd);
        let o = match cmd.output() {
            Ok(o) => o,
            Err(_) => {
                tried.push(exe);
                continue;
            }
        };
        // Windows' "App execution alias" stubs for python.exe / python3.exe exit with 9009 when no
        // Python is installed.
        if o.status.code() == Some(9009) {
            tried.push(exe);
            continue;
        }
        if o.status.success() {
            let code = std::fs::read_to_string(&out).map_err(|e| format!("cannot read the converted script: {e}"))?;
            let _ = std::fs::remove_file(&out);
            return Ok(code);
        }
        let _ = std::fs::remove_file(&out);
        let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
        return Err(if msg.is_empty() { format!("the converter failed ({})", o.status) } else { msg });
    }
    Err(format!("Python 3 was not found (tried {}). Install Python from python.org, then run: py -m pip install py-aep", tried.join(", ")))
}

fn pythons() -> Vec<(String, Vec<String>)> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("EFFECTCRAFT_PYTHON").filter(|p| !p.is_empty()) {
        v.push((p.to_string_lossy().into_owned(), vec![]));
    }
    if cfg!(windows) {
        v.push(("py".into(), vec!["-3".into()]));
    }
    v.push(("python3".into(), vec![]));
    v.push(("python".into(), vec![]));
    v
}

fn find_script() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("EFFECTCRAFT_AEP_SCRIPT").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().as_deref().and_then(Path::parent) {
        candidates.push(dir.join("aep-import").join(SCRIPT));
        // target/release or target/debug in a checkout.
        if let Some(root) = dir.parent().and_then(Path::parent) {
            candidates.push(root.join("tools").join("aep-import").join(SCRIPT));
        }
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("tools").join("aep-import").join(SCRIPT));
    candidates.into_iter().find(|p| p.is_file())
}

/// The converter is a console program; keep a window from flashing up behind the app.
#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_: &mut Command) {}
