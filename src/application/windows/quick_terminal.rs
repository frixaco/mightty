//! Native placement and visibility control for the persistent quick terminal.

use std::io;
use std::mem::size_of;

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOTOWNER, GetAncestor, GetCursorPos, GetForegroundWindow, HWND_TOPMOST, IsWindowVisible,
    SW_HIDE, SW_SHOW, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos, ShowWindow,
};

use crate::settings::QuickTerminalSettings;

/// Toggle the persistent quick-terminal window.
///
/// Returns `true` when the window is visible after the operation.
pub fn toggle_quick_terminal(
    window: &Window,
    settings: &QuickTerminalSettings,
) -> io::Result<bool> {
    let hwnd = native_window(window)?;
    if unsafe { IsWindowVisible(hwnd) } != 0 {
        hide(hwnd);
        Ok(false)
    } else {
        show(hwnd, settings)?;
        window.activate_window();
        Ok(true)
    }
}

/// Place, show, and focus the persistent quick-terminal window.
pub fn show_quick_terminal(window: &Window, settings: &QuickTerminalSettings) -> io::Result<()> {
    show(native_window(window)?, settings)?;
    window.activate_window();
    Ok(())
}

/// Hide the persistent quick-terminal window.
pub fn hide_quick_terminal(window: &Window) -> io::Result<()> {
    hide(native_window(window)?);
    Ok(())
}

/// Return whether the persistent quick-terminal window is visible.
pub fn quick_terminal_is_visible(window: &Window) -> io::Result<bool> {
    Ok(unsafe { IsWindowVisible(native_window(window)?) } != 0)
}

/// Return whether the foreground window belongs to the quick terminal.
///
/// Owned dialogs and menus count as part of the quick terminal.
pub fn quick_terminal_has_focus(window: &Window) -> io::Result<bool> {
    let hwnd = native_window(window)?;
    let foreground = unsafe { GetForegroundWindow() };
    Ok(!foreground.is_null()
        && (foreground == hwnd || unsafe { GetAncestor(foreground, GA_ROOTOWNER) } == hwnd))
}

fn show(hwnd: HWND, settings: &QuickTerminalSettings) -> io::Result<()> {
    let mut cursor = POINT::default();
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Err(io::Error::other("Windows did not return a display"));
    }

    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let placement = QuickTerminalPlacement::for_work_area(
        monitor_info.rcWork,
        settings.width_ratio,
        settings.height_ratio,
    );
    if unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            placement.x,
            placement.y,
            placement.width,
            placement.height,
            SWP_SHOWWINDOW,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
    Ok(())
}

fn hide(hwnd: HWND) {
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
}

fn native_window(window: &Window) -> io::Result<HWND> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| io::Error::other(format!("window handle unavailable: {error:?}")))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as HWND),
        _ => Err(io::Error::other("the window is not a Windows window")),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuickTerminalPlacement {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl QuickTerminalPlacement {
    fn for_work_area(work_area: RECT, width_ratio: f32, height_ratio: f32) -> Self {
        let available_width = work_area.right.saturating_sub(work_area.left).max(1);
        let available_height = work_area.bottom.saturating_sub(work_area.top).max(1);
        let width = ((available_width as f32) * width_ratio)
            .round()
            .clamp(1.0, available_width as f32) as i32;
        let height = ((available_height as f32) * height_ratio)
            .round()
            .clamp(1.0, available_height as f32) as i32;

        Self {
            x: work_area.left + (available_width - width) / 2,
            y: work_area.top,
            width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_the_quick_terminal_at_the_work_area_top() {
        let placement = QuickTerminalPlacement::for_work_area(
            RECT {
                left: -1920,
                top: 40,
                right: 0,
                bottom: 1080,
            },
            0.8,
            0.45,
        );

        assert_eq!(
            placement,
            QuickTerminalPlacement {
                x: -1728,
                y: 40,
                width: 1536,
                height: 468,
            }
        );
    }

    #[test]
    fn clamps_invalid_ratios_to_the_work_area() {
        let placement = QuickTerminalPlacement::for_work_area(
            RECT {
                left: 10,
                top: 20,
                right: 110,
                bottom: 70,
            },
            2.0,
            0.0,
        );

        assert_eq!(
            placement,
            QuickTerminalPlacement {
                x: 10,
                y: 20,
                width: 100,
                height: 1,
            }
        );
    }
}
