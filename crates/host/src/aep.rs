//! After Effects project import (`file.importAep`, the `aep` feature): runs the external converter
//! `tools/aep-import/aep2effectcraft.py`, which reads the `.aep` / `.aepx` with py-aep (MIT, a
//! Python package) and writes the After Effects-style script that rebuilds the project. Needs
//! Python 3 with py-aep installed (`py -m pip install py-aep`).
//!
//! Python, in this order: `EFFECTCRAFT_PYTHON`; the per-user environment in the config folder
//! (`<config dir>/aep-python/bin/python3`, `Scripts\python.exe` on Windows; the installers create it
//! where the system Python refuses `pip install --user`, PEP 668); `py -3` (the Windows launcher);
//! `python3`; on macOS `/opt/homebrew/bin/python3` and `/usr/local/bin/python3` (an app started from
//! Finder gets a minimal `PATH` without Homebrew); `python`. A Python that runs but has no py-aep (the
//! converter exits with 3) is passed over for the next one, unless `EFFECTCRAFT_PYTHON` named it. The
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
    convert_with(&pythons(), &script, path, &out)
}

/// Run the converter `script` on the project `path` with the first of `pythons` that can, writing
/// the generated script to `out`.
fn convert_with(pythons: &[Python], script: &Path, path: &str, out: &Path) -> Result<String, String> {
    let mut tried = Vec::new();
    // Pythons that ran but have no py-aep.
    let mut without = Vec::new();
    for py in pythons {
        let mut cmd = Command::new(&py.exe);
        cmd.args(&py.pre).arg(script).arg(path).arg("-o").arg(out);
        hide_console(&mut cmd);
        let o = match cmd.output() {
            Ok(o) => o,
            Err(_) => {
                tried.push(py.name());
                continue;
            }
        };
        // Windows' "App execution alias" stubs for python.exe / python3.exe exit with 9009 when no
        // Python is installed.
        if o.status.code() == Some(9009) {
            tried.push(py.name());
            continue;
        }
        if o.status.success() {
            let code = std::fs::read_to_string(out).map_err(|e| format!("cannot read the converted script: {e}"))?;
            let _ = std::fs::remove_file(out);
            return Ok(code);
        }
        let _ = std::fs::remove_file(out);
        // Exit code 3 is "py-aep is not installed": another Python found on this machine may have it.
        // The one the user named is the only one they want, so its failure is the answer.
        if o.status.code() == Some(CODE_NO_PY_AEP) && !py.explicit {
            without.push(py.name());
            continue;
        }
        let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
        return Err(if msg.is_empty() { format!("the converter failed ({})", o.status) } else { msg });
    }
    if let Some(first) = pythons.iter().find(|p| without.contains(&p.name())) {
        let install = if cfg!(windows) { format!("{} -m pip install py-aep", first.name()) } else { format!("{} -m pip install --user py-aep", first.name()) };
        return Err(format!(
            "py-aep is not installed for the Python found here ({}). Install it with:  {install}  (the installer, or \"Set up After Effects import\" on macOS, does this for you)",
            without.join(", ")
        ));
    }
    Err(format!(
        "Python 3 was not found (tried {}). Install Python from python.org, then run: {}",
        tried.join(", "),
        if cfg!(windows) { "py -m pip install py-aep" } else { "python3 -m pip install --user py-aep" }
    ))
}

/// What the converter exits with when py-aep cannot be imported (see `aep2effectcraft.py`).
const CODE_NO_PY_AEP: i32 = 3;

/// A Python to try: the program, the arguments before the script (`py -3`), and whether the user named
/// it with `EFFECTCRAFT_PYTHON`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Python {
    exe: String,
    pre: Vec<String>,
    explicit: bool,
}

impl Python {
    fn new(exe: impl Into<String>, pre: &[&str]) -> Python {
        Python { exe: exe.into(), pre: pre.iter().map(|s| s.to_string()).collect(), explicit: false }
    }

    /// The command as the user would type it (`py -3`), for messages.
    fn name(&self) -> String {
        std::iter::once(self.exe.as_str()).chain(self.pre.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

/// The operating systems whose Python locations differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Os {
    Windows,
    Mac,
    Other,
}

fn pythons() -> Vec<Python> {
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::Other
    };
    pythons_for(os, std::env::var_os("EFFECTCRAFT_PYTHON"), crate::config_dir().as_deref())
}

/// The folder of the per-user environment the installers make: `aep-python` in the config folder.
pub const VENV_DIR: &str = "aep-python";

/// The Python programs to try, in order (see the module docs).
fn pythons_for(os: Os, named: Option<std::ffi::OsString>, config_dir: Option<&Path>) -> Vec<Python> {
    let mut v = Vec::new();
    if let Some(p) = named.filter(|p| !p.is_empty()) {
        v.push(Python { exe: p.to_string_lossy().into_owned(), pre: vec![], explicit: true });
    }
    if let Some(dir) = config_dir {
        let venv = dir.join(VENV_DIR);
        let python = if os == Os::Windows { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python3") };
        v.push(Python::new(python.to_string_lossy().into_owned(), &[]));
    }
    if os == Os::Windows {
        v.push(Python::new("py", &["-3"]));
    }
    v.push(Python::new("python3", &[]));
    if os == Os::Mac {
        // An app started from Finder or the Dock gets a minimal PATH (/usr/bin:/bin:...) without Homebrew.
        v.push(Python::new("/opt/homebrew/bin/python3", &[]));
        v.push(Python::new("/usr/local/bin/python3", &[]));
    }
    v.push(Python::new("python", &[]));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[Python]) -> Vec<String> {
        v.iter().map(Python::name).collect()
    }

    /// The order of the Pythons per operating system: the named one, the config folder's environment,
    /// the launcher / PATH ones, and on macOS Homebrew's and the python.org links.
    #[test]
    fn the_pythons_are_tried_in_order() {
        let cfg = Path::new("/home/u/.config/effectcraft");
        let venv = cfg.join("aep-python").join("bin").join("python3").to_string_lossy().into_owned();
        assert_eq!(names(&pythons_for(Os::Other, None, Some(cfg))), [venv.as_str(), "python3", "python"]);
        assert_eq!(names(&pythons_for(Os::Mac, None, Some(cfg))), [venv.as_str(), "python3", "/opt/homebrew/bin/python3", "/usr/local/bin/python3", "python"]);
        let win = Path::new(r"C:\Users\u\AppData\Roaming\EffectCraft");
        let got = names(&pythons_for(Os::Windows, None, Some(win)));
        assert_eq!(got.len(), 4, "{got:?}");
        assert!(got[0].replace('/', "\\").ends_with(r"aep-python\Scripts\python.exe"), "{got:?}");
        assert_eq!(&got[1..], ["py -3", "python3", "python"]);
        // No config folder: no environment. EFFECTCRAFT_PYTHON comes first, explicit; empty is unset.
        assert_eq!(names(&pythons_for(Os::Other, None, None)), ["python3", "python"]);
        let named = pythons_for(Os::Mac, Some("/my/python".into()), Some(cfg));
        assert_eq!((named[0].exe.as_str(), named[0].explicit, named.iter().filter(|p| p.explicit).count()), ("/my/python", true, 1));
        assert_eq!(pythons_for(Os::Mac, Some("".into()), None).len(), 4);
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// A fake Python that behaves as `body` says; the converter's arguments are `<script> <project> -o <out>`.
    #[cfg(unix)]
    fn fakes(dir: &Path) -> (String, String, String) {
        let ok = script(dir, "ok.sh", r#"printf 'generated' > "$4"; exit 0"#);
        let no_aep = script(dir, "no-aep.sh", r#"echo 'py-aep is not installed.' >&2; exit 3"#);
        let broken = script(dir, "broken.sh", r#"echo 'bad project' >&2; exit 2"#);
        (ok, no_aep, broken)
    }

    /// A Python without py-aep is passed over for one that has it, and the answer names the Pythons
    /// when none has it; the Python the user named is the only one tried when it fails.
    #[cfg(unix)]
    #[test]
    fn a_python_without_py_aep_is_passed_over() {
        let dir = std::env::temp_dir().join(format!("effectcraft-aep-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (ok, no_aep, broken) = fakes(&dir);
        let out = dir.join("out.jsx");
        let py = |exe: &str, explicit: bool| Python { exe: exe.to_string(), pre: vec![], explicit };
        let run = |list: &[Python]| convert_with(list, Path::new("converter.py"), "project.aep", &out);
        // The first Python has no py-aep, the second does.
        assert_eq!(run(&[py(&no_aep, false), py(&ok, false)]), Ok("generated".to_string()));
        // One that is not there at all is skipped as well.
        assert_eq!(run(&[py("/no/such/python", false), py(&no_aep, false), py(&ok, false)]), Ok("generated".to_string()));
        // None has py-aep: one message that names them and says what to install.
        let e = run(&[py(&no_aep, false), py("/no/such/python", false)]).unwrap_err();
        assert!(e.contains("py-aep is not installed") && e.contains("no-aep.sh") && e.contains("pip install --user py-aep"), "{e}");
        // The named Python's own failure is the answer, even though another one would work.
        let e = run(&[py(&no_aep, true), py(&ok, false)]).unwrap_err();
        assert!(e.contains("py-aep is not installed."), "{e}");
        // Any other failure of the converter is final (a bad project is not a missing library).
        assert_eq!(run(&[py(&broken, false), py(&ok, false)]), Err("bad project".to_string()));
        // No Python at all.
        let e = run(&[py("/no/such/python", false), py("/no/other/python", false)]).unwrap_err();
        assert!(e.contains("Python 3 was not found") && e.contains("/no/such/python") && e.contains("/no/other/python"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
