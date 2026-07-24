use std::marker::PhantomData;
use std::mem::{MaybeUninit, size_of};
use std::ptr::NonNull;
use std::rc::Rc;

use crate::ghostty::error::{from_result, from_result_with_len};
use crate::ghostty::style::{RgbColor, Style};
use crate::ghostty::{Error, Result, Terminal, ffi};

/// Reusable render snapshot storage owned by mightty.
pub struct RenderState {
    raw: NonNull<ffi::RenderStateImpl>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// Read-only view of the latest state copied from a terminal.
pub struct Snapshot<'state> {
    state: &'state mut RenderState,
}

/// Reusable row iterator storage.
pub struct RowIterator {
    raw: NonNull<ffi::RenderStateRowIteratorImpl>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// Active lending iteration over a render snapshot's rows.
pub struct RowIteration<'iterator, 'snapshot> {
    iterator: &'iterator mut RowIterator,
    _snapshot: PhantomData<&'snapshot Snapshot<'snapshot>>,
}

/// Reusable cell iterator storage.
pub struct CellIterator {
    raw: NonNull<ffi::RenderStateRowCellsImpl>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// Active lending iteration over one row's cells.
pub struct CellIteration<'iterator, 'row> {
    iterator: &'iterator mut CellIterator,
    _row: PhantomData<&'row ()>,
}

impl RenderState {
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe { ffi::ghostty_render_state_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn update(&mut self, terminal: &Terminal) -> Result<Snapshot<'_>> {
        let result =
            unsafe { ffi::ghostty_render_state_update(self.raw.as_ptr(), terminal.as_raw()) };
        from_result(result)?;
        Ok(Snapshot { state: self })
    }
}

impl Snapshot<'_> {
    pub fn colors(&self) -> Result<Colors> {
        let mut raw = ffi::RenderStateColors {
            size: size_of::<ffi::RenderStateColors>(),
            ..Default::default()
        };
        let result =
            unsafe { ffi::ghostty_render_state_colors_get(self.state.raw.as_ptr(), &raw mut raw) };
        from_result(result)?;
        Ok(Colors {
            background: raw.background.into(),
            foreground: raw.foreground.into(),
            cursor: raw.cursor_has_value.then(|| raw.cursor.into()),
            palette: raw.palette.map(Into::into),
        })
    }

    pub fn cursor_viewport(&self) -> Result<Option<CursorViewport>> {
        if !unsafe { self.get_unchecked::<bool>(ffi::RenderStateData::CURSOR_VIEWPORT_HAS_VALUE) }?
        {
            return Ok(None);
        }
        Ok(Some(CursorViewport {
            x: unsafe { self.get_unchecked::<u16>(ffi::RenderStateData::CURSOR_VIEWPORT_X) }?,
            y: unsafe { self.get_unchecked::<u16>(ffi::RenderStateData::CURSOR_VIEWPORT_Y) }?,
            at_wide_tail: unsafe {
                self.get_unchecked::<bool>(ffi::RenderStateData::CURSOR_VIEWPORT_WIDE_TAIL)
            }?,
        }))
    }

    /// `T` must be the output type documented for `data`.
    unsafe fn get_unchecked<T>(&self, data: ffi::RenderStateData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::uninit();
        let result = unsafe {
            ffi::ghostty_render_state_get(self.state.raw.as_ptr(), data, value.as_mut_ptr().cast())
        };
        from_result(result)?;
        Ok(unsafe { value.assume_init() })
    }
}

impl RowIterator {
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_render_state_row_iterator_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn update<'iterator, 'snapshot>(
        &'iterator mut self,
        snapshot: &'snapshot Snapshot<'_>,
    ) -> Result<RowIteration<'iterator, 'snapshot>> {
        let mut raw = self.raw.as_ptr();
        let result = unsafe {
            ffi::ghostty_render_state_get(
                snapshot.state.raw.as_ptr(),
                ffi::RenderStateData::ROW_ITERATOR,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        from_result(result)?;
        self.raw = NonNull::new(raw).ok_or(Error::InvalidValue)?;
        Ok(RowIteration {
            iterator: self,
            _snapshot: PhantomData,
        })
    }
}

impl RowIteration<'_, '_> {
    #[expect(
        clippy::should_implement_trait,
        reason = "this is a lending iterator whose item borrows the iterator"
    )]
    pub fn next(&mut self) -> Option<&Self> {
        unsafe {
            ffi::ghostty_render_state_row_iterator_next(self.iterator.raw.as_ptr()).then_some(self)
        }
    }

    pub fn set_dirty(&self, dirty: bool) -> Result<()> {
        let result = unsafe {
            ffi::ghostty_render_state_row_set(
                self.iterator.raw.as_ptr(),
                ffi::RenderStateRowOption::DIRTY,
                std::ptr::from_ref(&dirty).cast(),
            )
        };
        from_result(result)
    }
}

impl CellIterator {
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_render_state_row_cells_new(std::ptr::null(), &raw mut raw) };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn update<'iterator, 'row>(
        &'iterator mut self,
        row: &'row RowIteration<'_, '_>,
    ) -> Result<CellIteration<'iterator, 'row>> {
        let mut raw = self.raw.as_ptr();
        let result = unsafe {
            ffi::ghostty_render_state_row_get(
                row.iterator.raw.as_ptr(),
                ffi::RenderStateRowData::CELLS,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        from_result(result)?;
        self.raw = NonNull::new(raw).ok_or(Error::InvalidValue)?;
        Ok(CellIteration {
            iterator: self,
            _row: PhantomData,
        })
    }
}

impl CellIteration<'_, '_> {
    #[expect(
        clippy::should_implement_trait,
        reason = "this is a lending iterator whose item borrows the iterator"
    )]
    pub fn next(&mut self) -> Option<&Self> {
        unsafe {
            ffi::ghostty_render_state_row_cells_next(self.iterator.raw.as_ptr()).then_some(self)
        }
    }

    pub fn width(&self) -> Result<CellWidth> {
        let cell = unsafe { self.get_unchecked::<ffi::Cell>(ffi::RenderStateRowCellsData::RAW) }?;
        let mut width = MaybeUninit::<ffi::CellWide::Type>::zeroed();
        let result =
            unsafe { ffi::ghostty_cell_get(cell, ffi::CellData::WIDE, width.as_mut_ptr().cast()) };
        from_result(result)?;
        CellWidth::from_raw(unsafe { width.assume_init() })
    }

    pub fn style(&self) -> Result<Style> {
        let mut raw = ffi::Style {
            size: size_of::<ffi::Style>(),
            ..Default::default()
        };
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iterator.raw.as_ptr(),
                ffi::RenderStateRowCellsData::STYLE,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        from_result(result)?;
        Style::from_raw(raw)
    }

    pub fn fg_color(&self) -> Result<Option<RgbColor>> {
        self.optional_color(ffi::RenderStateRowCellsData::FG_COLOR)
    }

    pub fn bg_color(&self) -> Result<Option<RgbColor>> {
        self.optional_color(ffi::RenderStateRowCellsData::BG_COLOR)
    }

    pub fn text(&self) -> Result<String> {
        let mut raw = ffi::Buffer::default();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iterator.raw.as_ptr(),
                ffi::RenderStateRowCellsData::GRAPHEMES_UTF8,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        let required = match from_result_with_len(result, raw.len) {
            Ok(0) => return Ok(String::new()),
            Ok(_) => return Err(Error::InvalidValue),
            Err(Error::OutOfSpace { required }) => required,
            Err(error) => return Err(error),
        };

        let mut bytes = vec![0_u8; required];
        raw.ptr = bytes.as_mut_ptr();
        raw.cap = bytes.len();
        raw.len = 0;
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iterator.raw.as_ptr(),
                ffi::RenderStateRowCellsData::GRAPHEMES_UTF8,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        let written = from_result_with_len(result, raw.len)?;
        if written > bytes.len() {
            return Err(Error::InvalidValue);
        }
        bytes.truncate(written);
        String::from_utf8(bytes).map_err(|_| Error::InvalidValue)
    }

    fn optional_color(&self, data: ffi::RenderStateRowCellsData::Type) -> Result<Option<RgbColor>> {
        match unsafe { self.get_unchecked::<ffi::ColorRgb>(data) } {
            Ok(color) => Ok(Some(color.into())),
            Err(Error::InvalidValue) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `T` must be the output type documented for `data`.
    unsafe fn get_unchecked<T>(&self, data: ffi::RenderStateRowCellsData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::uninit();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.iterator.raw.as_ptr(),
                data,
                value.as_mut_ptr().cast(),
            )
        };
        from_result(result)?;
        Ok(unsafe { value.assume_init() })
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_render_state_free(self.raw.as_ptr());
        }
    }
}

impl Drop for RowIterator {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_render_state_row_iterator_free(self.raw.as_ptr());
        }
    }
}

impl Drop for CellIterator {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_render_state_row_cells_free(self.raw.as_ptr());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellWidth {
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

impl CellWidth {
    fn from_raw(value: ffi::CellWide::Type) -> Result<Self> {
        match value {
            ffi::CellWide::NARROW => Ok(Self::Narrow),
            ffi::CellWide::WIDE => Ok(Self::Wide),
            ffi::CellWide::SPACER_TAIL => Ok(Self::SpacerTail),
            ffi::CellWide::SPACER_HEAD => Ok(Self::SpacerHead),
            _ => Err(Error::InvalidValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorViewport {
    pub x: u16,
    pub y: u16,
    pub at_wide_tail: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Colors {
    pub background: RgbColor,
    pub foreground: RgbColor,
    pub cursor: Option<RgbColor>,
    pub palette: [RgbColor; 256],
}
