use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::NonNull;

use crate::ghostty::error::from_result;
use crate::ghostty::{Error, Result, ffi};

/// Ghostty's reusable state machine for a terminal selection gesture.
pub(crate) struct SelectionGesture {
    raw: Option<NonNull<ffi::SelectionGestureImpl>>,
    press_event: GestureEvent,
    drag_event: GestureEvent,
    release_event: GestureEvent,
}

/// Pointer and terminal-grid position for a selection event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionPoint {
    pub column: u16,
    pub row: u32,
    pub surface_x: f64,
    pub surface_y: f64,
}

/// Display geometry Ghostty uses while interpreting a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionGeometry {
    pub columns: u32,
    pub cell_width: u32,
    pub screen_height: u32,
}

/// Input needed to begin a selection and classify repeated clicks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionPress {
    pub point: SelectionPoint,
    pub time_ns: u64,
    pub repeat_interval_ns: u64,
    pub repeat_distance: f64,
}

/// Input needed to update an active selection drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionDrag {
    pub point: SelectionPoint,
    pub geometry: SelectionGeometry,
    pub rectangle: bool,
}

pub(crate) enum SelectionUpdate {
    Set(ffi::Selection),
    Clear,
    Unchanged,
}

impl SelectionGesture {
    pub(crate) fn new() -> Result<Self> {
        let press_event = GestureEvent::new(ffi::SelectionGestureEventType::PRESS)?;
        let drag_event = GestureEvent::new(ffi::SelectionGestureEventType::DRAG)?;
        let release_event = GestureEvent::new(ffi::SelectionGestureEventType::RELEASE)?;

        let mut raw = std::ptr::null_mut();
        let result = unsafe { ffi::ghostty_selection_gesture_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;

        Ok(Self {
            raw: Some(NonNull::new(raw).ok_or(Error::InvalidValue)?),
            press_event,
            drag_event,
            release_event,
        })
    }

    pub(crate) fn press(
        &mut self,
        terminal: ffi::Terminal,
        input: SelectionPress,
    ) -> Result<SelectionUpdate> {
        let grid_ref = grid_ref(terminal, input.point)?;
        let position = surface_position(input.point);
        self.press_event
            .set(ffi::SelectionGestureEventOption::REF, &grid_ref)?;
        self.press_event
            .set(ffi::SelectionGestureEventOption::POSITION, &position)?;
        self.press_event.set(
            ffi::SelectionGestureEventOption::REPEAT_DISTANCE,
            &input.repeat_distance,
        )?;
        self.press_event
            .set(ffi::SelectionGestureEventOption::TIME_NS, &input.time_ns)?;
        self.press_event.set(
            ffi::SelectionGestureEventOption::REPEAT_INTERVAL_NS,
            &input.repeat_interval_ns,
        )?;

        match self.apply(terminal, &self.press_event)? {
            Some(selection) => Ok(SelectionUpdate::Set(selection)),
            None if self.click_count(terminal)? == 1 => Ok(SelectionUpdate::Clear),
            None => Ok(SelectionUpdate::Unchanged),
        }
    }

    pub(crate) fn drag(
        &mut self,
        terminal: ffi::Terminal,
        input: SelectionDrag,
    ) -> Result<SelectionUpdate> {
        let grid_ref = grid_ref(terminal, input.point)?;
        let position = surface_position(input.point);
        let geometry = ffi::SelectionGestureGeometry {
            columns: input.geometry.columns,
            cell_width: input.geometry.cell_width,
            padding_left: 0,
            screen_height: input.geometry.screen_height,
        };
        self.drag_event
            .set(ffi::SelectionGestureEventOption::REF, &grid_ref)?;
        self.drag_event
            .set(ffi::SelectionGestureEventOption::POSITION, &position)?;
        self.drag_event.set(
            ffi::SelectionGestureEventOption::RECTANGLE,
            &input.rectangle,
        )?;
        self.drag_event
            .set(ffi::SelectionGestureEventOption::GEOMETRY, &geometry)?;

        Ok(match self.apply(terminal, &self.drag_event)? {
            Some(selection) => SelectionUpdate::Set(selection),
            None => SelectionUpdate::Clear,
        })
    }

    pub(crate) fn release(
        &mut self,
        terminal: ffi::Terminal,
        point: Option<SelectionPoint>,
    ) -> Result<()> {
        if let Some(point) = point {
            let grid_ref = grid_ref(terminal, point)?;
            self.release_event
                .set(ffi::SelectionGestureEventOption::REF, &grid_ref)?;
        } else {
            self.release_event
                .clear(ffi::SelectionGestureEventOption::REF)?;
        }

        let _ = self.apply(terminal, &self.release_event)?;
        Ok(())
    }

    pub(crate) fn deinit(&mut self, terminal: ffi::Terminal) {
        if let Some(raw) = self.raw.take() {
            unsafe {
                ffi::ghostty_selection_gesture_free(raw.as_ptr(), terminal);
            }
        }
    }

    fn apply(
        &self,
        terminal: ffi::Terminal,
        event: &GestureEvent,
    ) -> Result<Option<ffi::Selection>> {
        let mut selection = ffi::Selection {
            size: size_of::<ffi::Selection>(),
            ..Default::default()
        };
        let result = unsafe {
            ffi::ghostty_selection_gesture_event(
                self.raw()?.as_ptr(),
                terminal,
                event.raw.as_ptr(),
                &raw mut selection,
            )
        };
        match result {
            ffi::Result::SUCCESS => Ok(Some(selection)),
            ffi::Result::NO_VALUE => Ok(None),
            other => {
                from_result(other)?;
                unreachable!("successful Ghostty result handled above")
            }
        }
    }

    fn click_count(&self, terminal: ffi::Terminal) -> Result<u8> {
        let mut click_count = 0_u8;
        let result = unsafe {
            ffi::ghostty_selection_gesture_get(
                self.raw()?.as_ptr(),
                terminal,
                ffi::SelectionGestureData::CLICK_COUNT,
                std::ptr::from_mut(&mut click_count).cast(),
            )
        };
        from_result(result)?;
        Ok(click_count)
    }

    fn raw(&self) -> Result<NonNull<ffi::SelectionGestureImpl>> {
        self.raw.ok_or(Error::InvalidValue)
    }
}

impl Drop for SelectionGesture {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            unsafe {
                ffi::ghostty_selection_gesture_free(raw.as_ptr(), std::ptr::null_mut());
            }
        }
    }
}

struct GestureEvent {
    raw: NonNull<ffi::SelectionGestureEventImpl>,
}

impl GestureEvent {
    fn new(event_type: ffi::SelectionGestureEventType::Type) -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_selection_gesture_event_new(std::ptr::null(), &raw mut raw, event_type)
        };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
        })
    }

    fn set<T>(&mut self, option: ffi::SelectionGestureEventOption::Type, value: &T) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.raw.as_ptr(),
                option,
                std::ptr::from_ref(value).cast::<c_void>(),
            )
        };
        from_result(result)
    }

    fn clear(&mut self, option: ffi::SelectionGestureEventOption::Type) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_selection_gesture_event_set(self.raw.as_ptr(), option, std::ptr::null())
        };
        from_result(result)
    }
}

impl Drop for GestureEvent {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_selection_gesture_event_free(self.raw.as_ptr());
        }
    }
}

fn grid_ref(terminal: ffi::Terminal, point: SelectionPoint) -> Result<ffi::GridRef> {
    let point = ffi::Point {
        tag: ffi::PointTag::VIEWPORT,
        value: ffi::PointValue {
            coordinate: ffi::PointCoordinate {
                x: point.column,
                y: point.row,
            },
        },
    };
    let mut grid_ref = ffi::GridRef {
        size: size_of::<ffi::GridRef>(),
        ..Default::default()
    };
    let result = unsafe { ffi::ghostty_terminal_grid_ref(terminal, point, &raw mut grid_ref) };
    from_result(result)?;
    Ok(grid_ref)
}

fn surface_position(point: SelectionPoint) -> ffi::SurfacePosition {
    ffi::SurfacePosition {
        x: point.surface_x,
        y: point.surface_y,
    }
}
