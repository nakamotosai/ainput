use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    let Some(rc) = find_rc_exe() else {
        println!("cargo:warning=rc.exe not found; Windows exe icon resource was not embedded");
        return;
    };
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("assets").join("app.ico");
    if !icon.exists() {
        println!(
            "cargo:warning=assets/app.ico not found; Windows exe icon resource was not embedded"
        );
        return;
    }
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rc_path = out_dir.join("ainput_icon.rc");
    let res_path = out_dir.join("ainput_icon.res");
    let icon_text = escape_rc_path(&icon);
    fs::write(&rc_path, format!("1 ICON \"{icon_text}\"\n")).expect("write icon rc file");
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg(format!("/fo{}", res_path.display()))
        .arg(&rc_path)
        .status()
        .expect("run rc.exe for icon resource");
    if !status.success() {
        panic!("rc.exe failed while compiling icon resource");
    }
    println!("cargo:rustc-link-arg={}", res_path.display());
}

fn escape_rc_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

fn find_rc_exe() -> Option<PathBuf> {
    find_in_path("rc.exe").or_else(find_windows_kit_rc)
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|path| path.join(name))
        .find(|path| path.exists())
}

fn find_windows_kit_rc() -> Option<PathBuf> {
    let root = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut candidates = Vec::new();
    collect_rc_candidates(&root, &mut candidates);
    candidates.sort_by(|left, right| right.cmp(left));
    candidates.into_iter().next()
}

fn collect_rc_candidates(root: &Path, candidates: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rc_candidates(&path, candidates);
            continue;
        }
        if path
            .file_name()
            .map(|name| name.to_string_lossy().eq_ignore_ascii_case("rc.exe"))
            .unwrap_or(false)
            && path
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().eq_ignore_ascii_case("x64"))
                .unwrap_or(false)
        {
            candidates.push(path);
        }
    }
}
