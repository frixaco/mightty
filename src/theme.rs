//! Cyberdream's default dark palette, shared by the app and terminal.
//! https://github.com/scottmckendry/cyberdream.nvim

use gpui_kit::component::{Theme, ThemeColor, ThemeMode};
use gpui_kit::{App, rgb};

pub(crate) const BG: u32 = 0x16181a;
pub(crate) const BG_ALT: u32 = 0x1e2124;
pub(crate) const BG_HIGHLIGHT: u32 = 0x3c4048;
pub(crate) const FG: u32 = 0xffffff;
pub(crate) const GREY: u32 = 0x7b8496;
pub(crate) const BLUE: u32 = 0x5ea1ff;
pub(crate) const GREEN: u32 = 0x5eff6c;
pub(crate) const CYAN: u32 = 0x5ef1ff;
pub(crate) const RED: u32 = 0xff6e5e;
pub(crate) const YELLOW: u32 = 0xf1ff5e;
pub(crate) const MAGENTA: u32 = 0xff5ef1;
pub(crate) const ORANGE: u32 = 0xffbd5e;
pub(crate) const PURPLE: u32 = 0xbd5eff;

// Window chrome around the terminal panes.
pub(crate) const SIDEBAR_BG: u32 = 0x111315;
pub(crate) const CHROME_BORDER: u32 = 0x23272b;
pub(crate) const DIVIDER: u32 = 0x262a2f;
pub(crate) const ROW_ACTIVE: u32 = 0x1f2226;
pub(crate) const ROW_ACTIVE_BORDER: u32 = 0x2b3036;
pub(crate) const PALETTE_BG: u32 = 0x1b1e21;
pub(crate) const KEYCAP: u32 = 0x2a2e34;
pub(crate) const FG_MUTED: u32 = 0xc9cfd8;
pub(crate) const GREY_DIM: u32 = 0x5d6574;
/// Focused-pane outline: `BLUE` at 55% alpha, as RGBA.
pub(crate) const FOCUS_RING: u32 = (BLUE << 8) | 0x8c;

// Matches Cyberdream's official Ghostty theme; ANSI magenta uses purple.
pub(crate) const ANSI: [u32; 16] = [
    BG,
    RED,
    GREEN,
    YELLOW,
    BLUE,
    PURPLE,
    CYAN,
    FG,
    BG_HIGHLIGHT,
    RED,
    GREEN,
    YELLOW,
    BLUE,
    PURPLE,
    CYAN,
    FG,
];

pub fn init(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| theme.colors = colors());
}

fn colors() -> ThemeColor {
    let bg = rgb(BG).into();
    let alt = rgb(BG_ALT).into();
    let highlight = rgb(BG_HIGHLIGHT).into();
    let fg = rgb(FG).into();
    let grey = rgb(GREY).into();
    let blue = rgb(BLUE).into();
    let green = rgb(GREEN).into();
    let cyan = rgb(CYAN).into();
    let red = rgb(RED).into();
    let yellow = rgb(YELLOW).into();
    let magenta = rgb(MAGENTA).into();
    ThemeColor {
        accent: highlight,
        accent_foreground: fg,
        accordion: alt,
        background: bg,
        border: highlight,
        button: alt,
        button_active: highlight,
        button_foreground: fg,
        button_hover: highlight,
        button_primary: blue,
        button_primary_active: cyan,
        button_primary_foreground: bg,
        button_primary_hover: cyan,
        button_secondary: alt,
        button_secondary_active: highlight,
        button_secondary_foreground: fg,
        button_secondary_hover: highlight,
        button_danger: red,
        button_danger_active: red,
        button_danger_foreground: bg,
        button_danger_hover: red,
        button_info: blue,
        button_info_active: blue,
        button_info_foreground: bg,
        button_info_hover: blue,
        button_success: green,
        button_success_active: green,
        button_success_foreground: bg,
        button_success_hover: green,
        button_warning: yellow,
        button_warning_active: yellow,
        button_warning_foreground: bg,
        button_warning_hover: yellow,
        group_box: alt,
        group_box_foreground: fg,
        caret: fg,
        chart_1: blue,
        chart_2: green,
        chart_3: cyan,
        chart_4: magenta,
        chart_5: yellow,
        danger: red,
        danger_active: red,
        danger_foreground: bg,
        danger_hover: red,
        description_list_label: alt,
        description_list_label_foreground: grey,
        drag_border: blue,
        drop_target: highlight,
        foreground: fg,
        info: blue,
        info_active: blue,
        info_foreground: bg,
        info_hover: blue,
        input: highlight,
        link: blue,
        link_active: cyan,
        link_hover: cyan,
        list: alt,
        list_active: highlight,
        list_active_border: blue,
        list_even: bg,
        list_head: alt,
        list_hover: highlight,
        muted: highlight,
        muted_foreground: grey,
        popover: alt,
        popover_foreground: fg,
        primary: blue,
        primary_active: cyan,
        primary_foreground: bg,
        primary_hover: cyan,
        progress_bar: blue,
        ring: blue,
        scrollbar: bg,
        scrollbar_thumb: highlight,
        scrollbar_thumb_hover: grey,
        secondary: alt,
        secondary_active: highlight,
        secondary_foreground: fg,
        secondary_hover: highlight,
        selection: highlight,
        sidebar: bg,
        sidebar_accent: highlight,
        sidebar_accent_foreground: fg,
        sidebar_border: highlight,
        sidebar_foreground: fg,
        sidebar_primary: blue,
        sidebar_primary_foreground: bg,
        skeleton: highlight,
        slider_bar: blue,
        slider_thumb: fg,
        success: green,
        success_foreground: bg,
        success_hover: green,
        success_active: green,
        chart_bullish: green,
        chart_bearish: red,
        chart_grid: highlight,
        switch: highlight,
        switch_thumb: fg,
        tab: bg,
        tab_active: highlight,
        tab_active_foreground: fg,
        tab_bar: bg,
        tab_bar_segmented: alt,
        tab_foreground: grey,
        table: bg,
        table_active: highlight,
        table_active_border: blue,
        table_even: alt,
        table_head: alt,
        table_head_foreground: fg,
        table_foot: alt,
        table_foot_foreground: fg,
        table_hover: highlight,
        table_row_border: highlight,
        title_bar: bg,
        title_bar_border: highlight,
        status_bar: bg,
        status_bar_border: highlight,
        warning: yellow,
        warning_active: yellow,
        warning_hover: yellow,
        warning_foreground: bg,
        overlay: gpui_kit::rgba((BG << 8) | 0x99).into(),
        window_border: highlight,
        red,
        red_light: red,
        green,
        green_light: green,
        blue,
        blue_light: blue,
        yellow,
        yellow_light: yellow,
        magenta,
        magenta_light: magenta,
        cyan,
        cyan_light: cyan,
    }
}
