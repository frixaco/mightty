use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "tools/ghostty-bindings/src/fingerprint.rs"]
mod fingerprint;

fn main() {
    let repo_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing manifest dir"));
    let ghostty_dir = repo_dir.join("ghostty");
    let bindings_version = repo_dir.join("src/ghostty/bindings.version");

    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/ghostty/bindings.version");
    println!("cargo:rerun-if-changed=ghostty/build.zig");
    println!("cargo:rerun-if-changed=ghostty/build.zig.zon");
    println!("cargo:rerun-if-changed=ghostty/include");
    println!("cargo:rerun-if-changed=ghostty/src");

    require_matching_bindings(&ghostty_dir, &bindings_version);
    build_and_link_ghostty(&ghostty_dir);
}

fn require_matching_bindings(ghostty_dir: &Path, version_path: &Path) {
    let build_zig = ghostty_dir.join("build.zig");
    assert!(
        build_zig.is_file(),
        "Ghostty source is missing at {}; run `git submodule update --init ghostty`",
        ghostty_dir.display()
    );

    let expected = fs::read_to_string(version_path).unwrap_or_else(|error| {
        panic!(
            "failed to read {}: {error}; regenerate with \
             `cargo run --manifest-path tools/ghostty-bindings/Cargo.toml`",
            version_path.display()
        )
    });
    let mut expected_lines = expected.lines();
    let expected_commit = expected_lines.next().unwrap_or_default();
    let expected_fingerprint = expected_lines.next().unwrap_or_default();
    assert!(
        !expected_commit.is_empty() && !expected_fingerprint.is_empty(),
        "{} must contain a Ghostty commit and header fingerprint",
        version_path.display()
    );

    let actual_commit = command_stdout(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(ghostty_dir),
        "identify the Ghostty submodule revision",
    );
    let git_head_path = command_stdout(
        Command::new("git")
            .args(["rev-parse", "--path-format=absolute", "--git-path", "HEAD"])
            .current_dir(ghostty_dir),
        "locate the Ghostty submodule HEAD",
    );
    println!("cargo:rerun-if-changed={git_head_path}");

    let actual_fingerprint = fingerprint::header_fingerprint(&ghostty_dir.join("include"));
    assert!(
        actual_commit == expected_commit && actual_fingerprint == expected_fingerprint,
        "Ghostty bindings do not match the checked-out source.\n\
         expected commit {expected_commit} ({expected_fingerprint})\n\
         actual commit   {actual_commit} ({actual_fingerprint})\n\
         Regenerate with `cargo run --manifest-path tools/ghostty-bindings/Cargo.toml`."
    );
    println!("cargo:rustc-env=MIGHTTY_GHOSTTY_REVISION={actual_commit}");
}

fn build_and_link_ghostty(ghostty_dir: &Path) {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("missing OUT_DIR"));
    let install_dir = out_dir.join("ghostty-install");
    let cache_dir = out_dir.join("ghostty-zig-cache");
    let target = env::var("TARGET").expect("missing TARGET");
    let host = env::var("HOST").expect("missing HOST");
    let optimize = optimize_mode();
    let zig = env::var_os("ZIG").unwrap_or_else(|| OsString::from("zig"));

    let mut command = Command::new(&zig);
    command
        .arg("build")
        .arg("-Demit-lib-vt=true")
        .arg("-Demit-xcframework=false")
        .arg("-Dapp-runtime=none")
        .arg(format!("-Doptimize={optimize}"))
        .arg("--prefix")
        .arg(&install_dir)
        .arg("--cache-dir")
        .arg(&cache_dir)
        .current_dir(ghostty_dir);
    if target != host {
        command.arg(format!("-Dtarget={}", zig_target(&target)));
    }
    run(&mut command, "build Ghostty's libghostty-vt with Zig");

    let installed_lib_dir = install_dir.join("lib");
    if target.contains("windows") {
        let source = installed_lib_dir.join("ghostty-vt-static.lib");
        assert!(
            source.is_file(),
            "Ghostty did not produce {}",
            source.display()
        );
        let rust_link_dir = out_dir.join("rust-link");
        fs::create_dir_all(&rust_link_dir).unwrap_or_else(|error| {
            panic!("failed to create {}: {error}", rust_link_dir.display())
        });
        let destination = rust_link_dir.join("ghostty-vt.lib");
        fs::copy(&source, &destination).unwrap_or_else(|error| {
            panic!(
                "failed to copy {} to {}: {error}",
                source.display(),
                destination.display()
            )
        });
        println!("cargo:rustc-link-search=native={}", rust_link_dir.display());
    } else {
        let source = installed_lib_dir.join("libghostty-vt.a");
        assert!(
            source.is_file(),
            "Ghostty did not produce {}",
            source.display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            installed_lib_dir.display()
        );
    }
    println!("cargo:rustc-link-lib=static=ghostty-vt");
}

fn optimize_mode() -> &'static str {
    if env::var("DEBUG").as_deref() == Ok("true") {
        "Debug"
    } else {
        match env::var("OPT_LEVEL").as_deref() {
            Ok("s" | "z") => "ReleaseSmall",
            _ => "ReleaseFast",
        }
    }
}

fn zig_target(target: &str) -> &'static str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "aarch64-apple-darwin" => "aarch64-macos-none",
        "x86_64-apple-darwin" => "x86_64-macos-none",
        "x86_64-pc-windows-gnu" => "x86_64-windows-gnu",
        "aarch64-pc-windows-gnullvm" => "aarch64-windows-gnu",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        "aarch64-linux-android" => "aarch64-linux-android",
        "x86_64-linux-android" => "x86_64-linux-android",
        other => panic!("unsupported Rust target for the Ghostty build: {other}"),
    }
}

fn command_stdout(command: &mut Command, context: &str) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to {context}: {error}"));
    assert!(
        output.status.success(),
        "failed to {context}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("{context} returned non-UTF-8 output: {error}"))
        .trim()
        .to_owned()
}

fn run(command: &mut Command, context: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to {context}: {error}"));
    assert!(status.success(), "failed to {context}: {status}");
}
