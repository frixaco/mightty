use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRED_ZIG_VERSION: &str = "0.15.2";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.mise.toml");
    println!("cargo:rerun-if-changed=ghostty/build.zig");
    println!("cargo:rerun-if-changed=ghostty/include/ghostty/vt.h");

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
    let ghostty_src = manifest_dir.join("ghostty");
    if !ghostty_src.join("build.zig").exists() {
        panic!(
            "ghostty checkout not found at {}. Sync the local ghostty tree first.",
            ghostty_src.display()
        );
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("missing OUT_DIR"));
    let install_dir = out_dir.join("ghostty-install");
    if install_dir.exists() {
        fs::remove_dir_all(&install_dir).expect("failed to remove previous ghostty install dir");
    }
    fs::create_dir_all(&install_dir).expect("failed to create ghostty install dir");

    let zig = zig_command(&manifest_dir);
    validate_zig_version(&zig);
    let status = Command::new(&zig)
        .args([
            "build",
            "-Demit-lib-vt=true",
            "-Dsimd=false",
            "--prefix",
            install_dir
                .to_str()
                .expect("ghostty install dir is not valid UTF-8"),
        ])
        .current_dir(&ghostty_src)
        .status()
        .expect("failed to run zig build; ensure Zig is installed and on PATH");

    if !status.success() {
        panic!("zig build failed for local ghostty checkout");
    }

    let lib_dir = install_dir.join("lib");
    let bin_dir = install_dir.join("bin");

    if env::var("TARGET")
        .expect("missing TARGET")
        .contains("windows")
    {
        copy_windows_dll(&bin_dir, &out_dir);
    } else {
        println!(
            "cargo:rustc-link-search=native={}",
            lib_dir.to_str().expect("lib dir is not valid UTF-8")
        );
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }
}

fn zig_command(project_dir: &Path) -> PathBuf {
    if let Some(path) = env::var_os("ZIG") {
        return PathBuf::from(path);
    }

    if let Ok(output) = Command::new("mise")
        .args(["which", "zig"])
        .current_dir(project_dir)
        .output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }

    for root in [env::var_os("LOCALAPPDATA"), env::var_os("USERPROFILE")] {
        let Some(root) = root else {
            continue;
        };

        let base = PathBuf::from(root);
        let candidate = if base.ends_with("Local") {
            base.join("mise")
                .join("installs")
                .join("zig")
                .join(REQUIRED_ZIG_VERSION)
                .join("zig.exe")
        } else {
            base.join("AppData")
                .join("Local")
                .join("mise")
                .join("installs")
                .join("zig")
                .join(REQUIRED_ZIG_VERSION)
                .join("zig.exe")
        };

        if candidate.exists() {
            return candidate;
        }
    }

    PathBuf::from("zig")
}

fn validate_zig_version(zig: &Path) {
    let output = Command::new(zig)
        .arg("version")
        .output()
        .unwrap_or_else(|err| panic!("failed to run {} version: {}", zig.display(), err));
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if !output.status.success() || version != REQUIRED_ZIG_VERSION {
        panic!(
            "mightty requires Zig {REQUIRED_ZIG_VERSION} to build ghostty, but {} reported {:?}. \
             Install it with `mise install zig@{REQUIRED_ZIG_VERSION}` or set ZIG to the correct zig.exe.",
            zig.display(),
            version
        );
    }
}

fn copy_windows_dll(bin_dir: &Path, out_dir: &Path) {
    let dll_src = bin_dir.join("ghostty-vt.dll");
    if !dll_src.exists() {
        panic!("expected ghostty-vt.dll at {}", dll_src.display());
    }

    let target_dir = out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("failed to determine target output directory");
    copy_dll(&dll_src, &target_dir.join("ghostty-vt.dll"));

    let deps_dir = target_dir.join("deps");
    if deps_dir.exists() {
        copy_dll(&dll_src, &deps_dir.join("ghostty-vt.dll"));
    }
}

fn copy_dll(src: &Path, dst: &Path) {
    fs::copy(src, dst).unwrap_or_else(|err| {
        panic!(
            "failed to copy {} to {}: {}",
            src.display(),
            dst.display(),
            err
        )
    });
}
