use std::collections::{HashMap, HashSet};
use std::ffi::CStr;

use serde::Deserialize;

use crate::ghostty::ffi;

macro_rules! assert_layout {
    ($layouts:ident, $checked:ident, $c_name:literal => $rust_type:ty {
        $($rust_field:ident => $c_field:literal),* $(,)?
    }) => {{
        let layout = $layouts
            .get($c_name)
            .unwrap_or_else(|| panic!("Ghostty ABI metadata is missing {}", $c_name));
        assert_eq!(
            std::mem::size_of::<$rust_type>(),
            layout.size,
            "{} size differs between Rust and Ghostty",
            $c_name
        );
        assert_eq!(
            std::mem::align_of::<$rust_type>(),
            layout.align,
            "{} alignment differs between Rust and Ghostty",
            $c_name
        );

        let expected_fields = HashSet::from([$($c_field),*]);
        let actual_fields = layout
            .fields
            .keys()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        assert_eq!(
            expected_fields,
            actual_fields,
            "{} fields differ between Rust and Ghostty",
            $c_name
        );
        $(
            assert_eq!(
                std::mem::offset_of!($rust_type, $rust_field),
                layout.fields[$c_field].offset,
                "{}.{} offset differs between Rust and Ghostty",
                $c_name,
                $c_field
            );
        )*
        assert!($checked.insert($c_name), "{} was checked twice", $c_name);
    }};
}

#[test]
fn rust_layouts_match_the_compiled_ghostty_library() {
    let pointer = unsafe { ffi::ghostty_type_json() };
    assert!(!pointer.is_null(), "ghostty_type_json returned null");
    let json = unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .expect("ghostty_type_json returned invalid UTF-8");
    let manifest: AbiManifest =
        serde_json::from_str(json).expect("ghostty_type_json returned invalid JSON");
    let layouts = manifest
        .types
        .into_iter()
        .filter(|(_, layout)| layout.kind == "struct")
        .collect::<HashMap<_, _>>();
    let mut checked = HashSet::new();

    assert_layout!(layouts, checked, "GhosttyAllocator" => ffi::Allocator {
        ctx => "ctx", vtable => "vtable"
    });
    assert_layout!(layouts, checked, "GhosttyAllocatorVtable" => ffi::AllocatorVtable {
        alloc => "alloc", resize => "resize", remap => "remap", free => "free"
    });
    assert_layout!(layouts, checked, "GhosttyBuffer" => ffi::Buffer {
        ptr => "ptr", cap => "cap", len => "len"
    });
    assert_layout!(layouts, checked, "GhosttyCellsView" => ffi::CellsView {
        ptr => "ptr", len => "len"
    });
    assert_layout!(layouts, checked, "GhosttyClipboardContent" => ffi::ClipboardContent {
        mime => "mime", data => "data"
    });
    assert_layout!(layouts, checked, "GhosttyClipboardWrite" => ffi::ClipboardWrite {
        size => "size", location => "location", contents => "contents", contents_len => "contents_len"
    });
    assert_layout!(layouts, checked, "GhosttyCodepoints" => ffi::Codepoints {
        ptr => "ptr", len => "len"
    });
    assert_layout!(layouts, checked, "GhosttyColorPaletteMask" => ffi::ColorPaletteMask {
        bits => "bits"
    });
    assert_layout!(layouts, checked, "GhosttyColorRgb" => ffi::ColorRgb {
        r => "r", g => "g", b => "b"
    });
    assert_layout!(layouts, checked, "GhosttyColorX11Entry" => ffi::ColorX11Entry {
        name => "name", color => "color"
    });
    assert_layout!(layouts, checked, "GhosttyDeviceAttributes" => ffi::DeviceAttributes {
        primary => "primary", secondary => "secondary", tertiary => "tertiary"
    });
    assert_layout!(layouts, checked, "GhosttyDeviceAttributesPrimary" => ffi::DeviceAttributesPrimary {
        conformance_level => "conformance_level", features => "features", num_features => "num_features"
    });
    assert_layout!(layouts, checked, "GhosttyDeviceAttributesSecondary" => ffi::DeviceAttributesSecondary {
        device_type => "device_type", firmware_version => "firmware_version", rom_cartridge => "rom_cartridge"
    });
    assert_layout!(layouts, checked, "GhosttyDeviceAttributesTertiary" => ffi::DeviceAttributesTertiary {
        unit_id => "unit_id"
    });
    assert_layout!(layouts, checked, "GhosttyFormatterScreenExtra" => ffi::FormatterScreenExtra {
        size => "size", cursor => "cursor", style => "style", hyperlink => "hyperlink",
        protection => "protection", kitty_keyboard => "kitty_keyboard", charsets => "charsets"
    });
    assert_layout!(layouts, checked, "GhosttyFormatterTerminalExtra" => ffi::FormatterTerminalExtra {
        size => "size", palette => "palette", modes => "modes",
        scrolling_region => "scrolling_region", tabstops => "tabstops", pwd => "pwd",
        keyboard => "keyboard", screen => "screen"
    });
    assert_layout!(layouts, checked, "GhosttyFormatterTerminalOptions" => ffi::FormatterTerminalOptions {
        size => "size", emit => "emit", unwrap => "unwrap", trim => "trim",
        extra => "extra", selection => "selection"
    });
    assert_layout!(layouts, checked, "GhosttyKittyGraphicsPlacementRenderInfo" => ffi::KittyGraphicsPlacementRenderInfo {
        size => "size", pixel_width => "pixel_width", pixel_height => "pixel_height",
        grid_cols => "grid_cols", grid_rows => "grid_rows",
        viewport_col => "viewport_col", viewport_row => "viewport_row",
        viewport_visible => "viewport_visible", source_x => "source_x", source_y => "source_y",
        source_width => "source_width", source_height => "source_height"
    });
    assert_layout!(layouts, checked, "GhosttyGridRef" => ffi::GridRef {
        size => "size", node => "node", x => "x", y => "y"
    });
    assert_layout!(layouts, checked, "GhosttyMouseEncoderSize" => ffi::MouseEncoderSize {
        size => "size", screen_width => "screen_width", screen_height => "screen_height",
        cell_width => "cell_width", cell_height => "cell_height",
        padding_top => "padding_top", padding_bottom => "padding_bottom",
        padding_right => "padding_right", padding_left => "padding_left"
    });
    assert_layout!(layouts, checked, "GhosttyMousePosition" => ffi::MousePosition {
        x => "x", y => "y"
    });
    assert_layout!(layouts, checked, "GhosttyPoint" => ffi::Point {
        tag => "tag", value => "value"
    });
    assert_layout!(layouts, checked, "GhosttyPointCoordinate" => ffi::PointCoordinate {
        x => "x", y => "y"
    });
    assert_layout!(layouts, checked, "GhosttyReader" => ffi::Reader {
        read => "read", userdata => "userdata"
    });
    assert_layout!(layouts, checked, "GhosttyRenderStateColors" => ffi::RenderStateColors {
        size => "size", background => "background", foreground => "foreground",
        cursor => "cursor", cursor_has_value => "cursor_has_value", palette => "palette"
    });
    assert_layout!(layouts, checked, "GhosttyRenderStateCursor" => ffi::RenderStateCursor {
        size => "size", viewport_has_value => "viewport_has_value",
        viewport_x => "viewport_x", viewport_y => "viewport_y", wide_tail => "wide_tail",
        visible => "visible", blinking => "blinking", password_input => "password_input",
        visual_style => "visual_style"
    });
    assert_layout!(layouts, checked, "GhosttyRenderStateRowSelection" => ffi::RenderStateRowSelection {
        size => "size", start_x => "start_x", end_x => "end_x"
    });
    assert_layout!(layouts, checked, "GhosttySelection" => ffi::Selection {
        size => "size", start => "start", end => "end", rectangle => "rectangle"
    });
    assert_layout!(layouts, checked, "GhosttySelectionGestureBehaviors" => ffi::SelectionGestureBehaviors {
        single_click => "single_click", double_click => "double_click", triple_click => "triple_click"
    });
    assert_layout!(layouts, checked, "GhosttySelectionGestureGeometry" => ffi::SelectionGestureGeometry {
        columns => "columns", cell_width => "cell_width",
        padding_left => "padding_left", screen_height => "screen_height"
    });
    assert_layout!(layouts, checked, "GhosttySgrAttribute" => ffi::SgrAttribute {
        tag => "tag", value => "value"
    });
    assert_layout!(layouts, checked, "GhosttySgrUnknown" => ffi::SgrUnknown {
        full_ptr => "full_ptr", full_len => "full_len",
        partial_ptr => "partial_ptr", partial_len => "partial_len"
    });
    assert_layout!(layouts, checked, "GhosttySizeReportSize" => ffi::SizeReportSize {
        rows => "rows", columns => "columns",
        cell_width => "cell_width", cell_height => "cell_height"
    });
    assert_layout!(layouts, checked, "GhosttyString" => ffi::String {
        ptr => "ptr", len => "len"
    });
    assert_layout!(layouts, checked, "GhosttyStyle" => ffi::Style {
        size => "size", fg_color => "fg_color", bg_color => "bg_color",
        underline_color => "underline_color", bold => "bold", italic => "italic",
        faint => "faint", blink => "blink", inverse => "inverse", invisible => "invisible",
        strikethrough => "strikethrough", overline => "overline", underline => "underline"
    });
    assert_layout!(layouts, checked, "GhosttyStyleColor" => ffi::StyleColor {
        tag => "tag", value => "value"
    });
    assert_layout!(layouts, checked, "GhosttySurfacePosition" => ffi::SurfacePosition {
        x => "x", y => "y"
    });
    assert_layout!(layouts, checked, "GhosttySysImage" => ffi::SysImage {
        width => "width", height => "height", data => "data", data_len => "data_len"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalDesktopNotification" => ffi::TerminalDesktopNotification {
        size => "size", title => "title", body => "body"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalModeConfig" => ffi::TerminalModeConfig {
        mode => "mode", value => "value"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalProgressReport" => ffi::TerminalProgressReport {
        size => "size", state => "state", progress => "progress"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalScrollViewport" => ffi::TerminalScrollViewport {
        tag => "tag", value => "value"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalScrollbar" => ffi::TerminalScrollbar {
        total => "total", offset => "offset", len => "len"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalSelectionFormatOptions" => ffi::TerminalSelectionFormatOptions {
        size => "size", emit => "emit", unwrap => "unwrap", trim => "trim", selection => "selection"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalSelectLineOptions" => ffi::TerminalSelectLineOptions {
        size => "size", ref_ => "ref", whitespace => "whitespace",
        whitespace_len => "whitespace_len", semantic_prompt_boundary => "semantic_prompt_boundary"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalSelectWordBetweenOptions" => ffi::TerminalSelectWordBetweenOptions {
        size => "size", start => "start", end => "end",
        boundary_codepoints => "boundary_codepoints",
        boundary_codepoints_len => "boundary_codepoints_len"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalSelectWordOptions" => ffi::TerminalSelectWordOptions {
        size => "size", ref_ => "ref", boundary_codepoints => "boundary_codepoints",
        boundary_codepoints_len => "boundary_codepoints_len"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalUnknownSequence" => ffi::TerminalUnknownSequence {
        tag => "tag", value => "value"
    });
    assert_layout!(layouts, checked, "GhosttyTerminalUnknownStringSequence" => ffi::TerminalUnknownStringSequence {
        truncated => "truncated", content => "content"
    });
    assert_layout!(layouts, checked, "GhosttyWriter" => ffi::Writer {
        write => "write", userdata => "userdata"
    });

    assert_eq!(
        checked.len(),
        layouts.len(),
        "Ghostty added or removed a public C struct; update this ABI audit"
    );
}

#[derive(Deserialize)]
struct AbiManifest {
    types: HashMap<String, AbiLayout>,
}

#[derive(Deserialize)]
struct AbiLayout {
    kind: String,
    size: usize,
    align: usize,
    #[serde(default)]
    fields: HashMap<String, AbiField>,
}

#[derive(Deserialize)]
struct AbiField {
    offset: usize,
}
