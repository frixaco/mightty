use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Only rebuild if this build.rs changes
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ghostty_src = out_dir.join("ghostty-src");
    let ghostty_install = out_dir.join("ghostty-install");

    // Clone ghostty if not already present
    if !ghostty_src.join("build.zig").exists() {
        println!("cargo:warning=Cloning ghostty repository...");
        let status = Command::new("git")
            .args(&[
                "clone",
                "--depth",
                "1",
                "https://github.com/ghostty-org/ghostty.git",
                ghostty_src.to_str().unwrap(),
            ])
            .status()
            .expect("Failed to clone ghostty repository. Make sure git is installed.");

        if !status.success() {
            panic!("Failed to clone ghostty repository");
        }
    }

    // Build libghostty-vt using zig (native target)
    println!("cargo:warning=Building libghostty-vt with Zig...");
    let status = Command::new("zig")
        .args(&[
            "build",
            "-Demit-lib-vt=true",
            "-Dsimd=false",
            "--prefix",
            ghostty_install.to_str().unwrap(),
        ])
        .current_dir(&ghostty_src)
        .status()
        .expect("Failed to run zig build. Make sure zig is installed and on PATH.");

    if !status.success() {
        eprintln!("\n=== Build Error ===");
        eprintln!("The Zig build failed. This is a known issue on Windows.");
        eprintln!("\nWorkaround for Windows:");
        eprintln!("1. Use WSL2 (Windows Subsystem for Linux)");
        eprintln!("2. Or use a pre-built library (see README.md)");
        eprintln!("3. Or build manually with MSVC and copy the library");
        eprintln!("\nFor detailed instructions, see README.md");
        panic!("zig build failed - see error message above");
    }

    // Link the library
    let lib_dir = ghostty_install.join("lib");
    let bin_dir = ghostty_install.join("bin");

    // On Windows, we use raw-dylib linking in the source code, so we don't
    // need to link here. Just copy the DLL to the target directory.
    if env::var("TARGET").unwrap().contains("windows") {
        // Copy DLL to target directory so it can be found at runtime
        let dll_src = bin_dir.join("ghostty-vt.dll");
        // Get the target directory from OUT_DIR (go up to target/debug or target/release)
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let target_dir = out_dir
            .parent() // build/<hash>/out -> build/<hash>
            .and_then(|p| p.parent()) // build/<hash> -> build
            .and_then(|p| p.parent()) // build -> debug or release
            .expect("Failed to determine target directory");
        let dll_dst = target_dir.join("ghostty-vt.dll");
        if dll_src.exists() {
            std::fs::copy(&dll_src, &dll_dst).expect("Failed to copy DLL");
            println!(
                "cargo:warning=Copied ghostty-vt.dll to {}",
                dll_dst.display()
            );
        }
    } else {
        println!(
            "cargo:rustc-link-search=native={}",
            lib_dir.to_str().unwrap()
        );
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }

    // Include headers for bindgen (if using bindgen)
    let include_dir = ghostty_install.join("include");
    println!("cargo:include={}", include_dir.to_str().unwrap());
}
