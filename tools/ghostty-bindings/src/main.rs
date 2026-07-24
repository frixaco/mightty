use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use bindgen::EnumVariation;
use bindgen::callbacks::{EnumVariantValue, IntKind, ItemInfo, ItemKind, ParseCallbacks};
use heck::ToShoutySnakeCase;

mod fingerprint;

fn main() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("binding tool must live at tools/ghostty-bindings")
        .to_path_buf();
    let ghostty_dir = repo_dir.join("ghostty");
    let include_dir = ghostty_dir.join("include");
    let header = include_dir.join("ghostty").join("vt.h");
    let output = repo_dir.join("src").join("ghostty").join("ffi.rs");
    let version_output = repo_dir
        .join("src")
        .join("ghostty")
        .join("bindings.version");

    assert!(
        header.is_file(),
        "Ghostty headers are missing at {}; initialize the ghostty submodule",
        header.display()
    );

    let commit = ghostty_commit(&ghostty_dir);
    let fingerprint = fingerprint::header_fingerprint(&include_dir);
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.to_string_lossy()))
        .allowlist_function("[Gg]hostty.*")
        .allowlist_type("[Gg]hostty.*")
        .allowlist_var("GHOSTTY_.*")
        .generate_cstr(true)
        .generate_comments(false)
        .derive_default(true)
        .size_t_is_usize(true)
        .layout_tests(false)
        .default_enum_style(EnumVariation::ModuleConsts)
        .raw_line(format!(
            "// Ghostty source: {commit}; public-header fingerprint: {fingerprint}"
        ))
        .parse_callbacks(Box::new(Callbacks))
        .generate()
        .expect("failed to generate Rust bindings from Ghostty's public C header");

    bindings
        .write_to_file(&output)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", output.display()));
    fs::write(&version_output, format!("{commit}\n{fingerprint}\n"))
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", version_output.display()));

    println!(
        "generated {} from Ghostty {} ({})",
        output.display(),
        commit,
        fingerprint
    );
}

fn ghostty_commit(ghostty_dir: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(ghostty_dir)
        .output()
        .expect("failed to run git while identifying the Ghostty source");
    assert!(
        output.status.success(),
        "git rev-parse failed for {}",
        ghostty_dir.display()
    );
    String::from_utf8(output.stdout)
        .expect("Ghostty commit is not UTF-8")
        .trim()
        .to_owned()
}

const PREFIXES: &[(&str, &str)] = &[
    ("GhosttyOptimizeMode", "GHOSTTY_OPTIMIZE"),
    ("GhosttyKeyEncoderOption", "GHOSTTY_KEY_ENCODER_OPT"),
    ("GhosttyMouseTrackingMode", "GHOSTTY_MOUSE_TRACKING"),
    ("GhosttyMouseEncoderOption", "GHOSTTY_MOUSE_ENCODER_OPT"),
    ("GhosttySgrAttributeTag", "GHOSTTY_SGR_ATTR"),
    ("GhosttyOscCommandData", "GHOSTTY_OSC_DATA"),
    ("GhosttyOscCommandType", "GHOSTTY_OSC_COMMAND"),
    ("GhosttyTerminalOption", "GHOSTTY_TERMINAL_OPT"),
    (
        "GhosttyTerminalScrollViewportTag",
        "GHOSTTY_SCROLL_VIEWPORT",
    ),
    ("GhosttyStyleColorTag", "GHOSTTY_STYLE_COLOR"),
    ("GhosttyRowSemanticPrompt", "GHOSTTY_ROW_SEMANTIC"),
    ("GhosttyCellSemanticContent", "GHOSTTY_CELL_SEMANTIC"),
    ("GhosttyCellContentTag", "GHOSTTY_CELL_CONTENT"),
    ("GhosttySizeReportStyle", "GHOSTTY_SIZE_REPORT"),
    ("GhosttyModeReportState", "GHOSTTY_MODE_REPORT"),
    ("GhosttyFocusEvent", "GHOSTTY_FOCUS"),
    ("GhosttyResult", "GHOSTTY_"),
    ("GhosttyKittyGraphicsImageData", "GHOSTTY_KITTY_IMAGE_DATA"),
    (
        "GhosttySelectionGestureEventOption",
        "GHOSTTY_SELECTION_GESTURE_EVENT_OPT",
    ),
];

#[derive(Debug)]
struct Callbacks;

impl ParseCallbacks for Callbacks {
    fn item_name(&self, item_info: ItemInfo) -> Option<String> {
        let prefix = match item_info.kind {
            ItemKind::Function => return None,
            ItemKind::Var => "GHOSTTY_",
            _ => "Ghostty",
        };
        Some(item_info.name.trim_start_matches(prefix).to_string())
    }

    fn enum_variant_name(
        &self,
        enum_name: Option<&str>,
        original_variant_name: &str,
        _variant_value: EnumVariantValue,
    ) -> Option<String> {
        let enum_name = enum_name?;
        let prefix = PREFIXES
            .iter()
            .find(|(value, _)| *value == enum_name)
            .map_or_else(
                || enum_name.to_shouty_snake_case(),
                |(_, name)| (*name).to_owned(),
            );
        Some(
            original_variant_name
                .trim_start_matches(&prefix)
                .trim_start_matches('_')
                .to_owned(),
        )
    }

    fn int_macro(&self, name: &str, _value: i64) -> Option<IntKind> {
        if name.starts_with("GHOSTTY_DA_") || name.starts_with("GHOSTTY_MODS_") {
            Some(IntKind::U16)
        } else if name.starts_with("GHOSTTY_KITTY_KEY_") || name.starts_with("GHOSTTY_COLOR_NAMED_")
        {
            Some(IntKind::U8)
        } else {
            None
        }
    }
}
