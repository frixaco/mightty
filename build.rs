use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ghostty/build.zig");
    println!("cargo:rerun-if-changed=ghostty/include/ghostty/vt.h");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
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
        let _ = fs::remove_dir_all(&install_dir);
    }
    fs::create_dir_all(&install_dir).expect("failed to create ghostty install dir");

    let status = Command::new("zig")
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
    let dll_dst = target_dir.join("ghostty-vt.dll");

    fs::copy(&dll_src, &dll_dst).unwrap_or_else(|err| {
        panic!(
            "failed to copy {} to {}: {}",
            dll_src.display(),
            dll_dst.display(),
            err
        )
    });
}
