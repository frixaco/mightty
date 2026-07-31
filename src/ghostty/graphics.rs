use std::ffi::c_void;
use std::io::Cursor;
use std::marker::PhantomData;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ghostty::error::from_result;
use crate::ghostty::{Error, Result, Terminal, ffi};

const MAX_PNG_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PNG_DIMENSION: u32 = 16_384;
static PNG_DECODE_LIMIT: AtomicU64 = AtomicU64::new(1);
static PNG_DECODER: OnceLock<Result<()>> = OnceLock::new();

/// Borrowed Kitty graphics storage for one terminal screen.
///
/// Ghostty invalidates this storage on the next terminal mutation. The terminal
/// borrow prevents safe Rust callers from mutating the terminal while using it.
pub struct Graphics<'terminal> {
    raw: NonNull<ffi::KittyGraphicsImpl>,
    terminal: &'terminal Terminal,
}

/// Reusable storage for iterating Kitty graphics placements.
pub struct PlacementIterator {
    raw: NonNull<ffi::KittyGraphicsPlacementIteratorImpl>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// Active lending iteration over placements from one graphics storage.
pub struct Placements<'iterator, 'graphics, 'terminal> {
    iterator: &'iterator mut PlacementIterator,
    graphics: &'graphics Graphics<'terminal>,
}

/// One placement and its borrowed image.
///
/// The placement borrows the active iteration. Callers must release it before
/// advancing the iterator.
pub struct Placement<'placement> {
    pub image_id: u32,
    pub placement_id: u32,
    pub is_virtual: bool,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z: i32,
    pub geometry: PlacementGeometry,
    pub image: Image<'placement>,
}

/// Borrowed decoded pixels for one stored image.
pub struct Image<'image> {
    raw: NonNull<ffi::KittyGraphicsImageImpl>,
    _graphics: PhantomData<&'image Terminal>,
}

/// Decoded image data that can be uploaded directly to a GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageData<'image> {
    pub id: u32,
    pub number: u32,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub generation: u64,
    pub pixels: &'image [u8],
}

/// Decoded pixel layout used by a stored image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Rgba,
    GrayAlpha,
    Gray,
}

/// Placement groups relative to cell backgrounds and terminal text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlacementLayer {
    #[default]
    All,
    BelowBackground,
    BelowText,
    AboveText,
}

/// Resolved placement geometry for the terminal's current viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacementGeometry {
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub grid_columns: u32,
    pub grid_rows: u32,
    pub viewport_column: i32,
    pub viewport_row: i32,
    pub viewport_visible: bool,
    pub source: SourceRect,
}

/// Resolved image crop in source pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Terminal {
    /// Enables Kitty graphics storage while keeping all external media disabled.
    ///
    /// Kitty direct media carries image bytes in the terminal stream. File,
    /// temporary-file, and shared-memory media remain disabled.
    pub fn enable_direct_graphics(&mut self, storage_limit: u64) -> Result<&mut Self> {
        if storage_limit == 0 {
            return Err(Error::InvalidValue);
        }
        install_png_decoder(storage_limit)?;

        let disabled = false;
        for option in [
            ffi::TerminalOption::KITTY_IMAGE_MEDIUM_FILE,
            ffi::TerminalOption::KITTY_IMAGE_MEDIUM_SHARED_MEM,
        ] {
            let result = unsafe {
                ffi::ghostty_terminal_set(
                    self.as_raw(),
                    option,
                    std::ptr::from_ref(&disabled).cast(),
                )
            };
            from_result(result)?;
        }

        let result = unsafe {
            ffi::ghostty_terminal_set(
                self.as_raw(),
                ffi::TerminalOption::KITTY_IMAGE_MEDIUM_TEMP_FILE,
                std::ptr::null(),
            )
        };
        from_result(result)?;

        let result = unsafe {
            ffi::ghostty_terminal_set(
                self.as_raw(),
                ffi::TerminalOption::KITTY_IMAGE_STORAGE_LIMIT,
                std::ptr::from_ref(&storage_limit).cast(),
            )
        };
        from_result(result)?;
        Ok(self)
    }

    /// Borrows the Kitty graphics storage for the active terminal screen.
    pub fn graphics(&self) -> Result<Graphics<'_>> {
        let mut raw: ffi::KittyGraphics = std::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::TerminalData::KITTY_GRAPHICS,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        from_result(result)?;
        Ok(Graphics {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            terminal: self,
        })
    }
}

fn install_png_decoder(storage_limit: u64) -> Result<()> {
    PNG_DECODE_LIMIT.fetch_max(storage_limit.min(MAX_PNG_DECODE_BYTES), Ordering::Relaxed);
    *PNG_DECODER.get_or_init(|| {
        let callback = decode_png as *const () as *const c_void;
        let result =
            unsafe { ffi::ghostty_sys_set(ffi::SysOption::GHOSTTY_SYS_OPT_DECODE_PNG, callback) };
        from_result(result)
    })
}

unsafe extern "C" fn decode_png(
    _userdata: *mut c_void,
    allocator: *const ffi::Allocator,
    data: *const u8,
    data_len: usize,
    out: *mut ffi::SysImage,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        decode_png_inner(allocator, data, data_len, out)
    }))
    .unwrap_or(false)
}

fn decode_png_inner(
    allocator: *const ffi::Allocator,
    data: *const u8,
    data_len: usize,
    out: *mut ffi::SysImage,
) -> bool {
    let decode_limit = PNG_DECODE_LIMIT.load(Ordering::Relaxed);
    if allocator.is_null()
        || data.is_null()
        || out.is_null()
        || data_len == 0
        || u64::try_from(data_len).map_or(true, |len| len > MAX_PNG_DECODE_BYTES)
    {
        return false;
    }

    let bytes = unsafe { std::slice::from_raw_parts(data, data_len) };
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PNG_DIMENSION);
    limits.max_image_height = Some(MAX_PNG_DIMENSION);
    limits.max_alloc = Some(decode_limit.saturating_mul(2));
    reader.limits(limits);
    let Ok(decoded) = reader.decode() else {
        return false;
    };
    let pixels = decoded.into_rgba8();
    let (width, height) = pixels.dimensions();
    let pixels = pixels.into_raw();
    if width == 0
        || height == 0
        || u64::try_from(pixels.len()).map_or(true, |len| len > decode_limit)
    {
        return false;
    }

    let output = unsafe { ffi::ghostty_alloc(allocator, pixels.len()) };
    if output.is_null() {
        return false;
    }
    unsafe {
        output.copy_from_nonoverlapping(pixels.as_ptr(), pixels.len());
        out.write(ffi::SysImage {
            width,
            height,
            data: output,
            data_len: pixels.len(),
        });
    }
    true
}

impl<'terminal> Graphics<'terminal> {
    /// Returns the storage-wide content generation.
    pub fn generation(&self) -> Result<u64> {
        let mut generation = 0_u64;
        let result = unsafe {
            ffi::ghostty_kitty_graphics_get(
                self.raw.as_ptr(),
                ffi::KittyGraphicsData::GENERATION,
                std::ptr::from_mut(&mut generation).cast(),
            )
        };
        from_result(result)?;
        Ok(generation)
    }

    /// Populates an iterator and applies one z-layer filter.
    pub fn placements<'iterator, 'graphics>(
        &'graphics self,
        iterator: &'iterator mut PlacementIterator,
        layer: PlacementLayer,
    ) -> Result<Placements<'iterator, 'graphics, 'terminal>> {
        let mut raw = iterator.raw.as_ptr();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_get(
                self.raw.as_ptr(),
                ffi::KittyGraphicsData::PLACEMENT_ITERATOR,
                std::ptr::from_mut(&mut raw).cast(),
            )
        };
        from_result(result)?;
        iterator.raw = NonNull::new(raw).ok_or(Error::InvalidValue)?;

        let layer = layer.as_raw();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_iterator_set(
                iterator.raw.as_ptr(),
                ffi::KittyGraphicsPlacementIteratorOption::LAYER,
                std::ptr::from_ref(&layer).cast(),
            )
        };
        from_result(result)?;
        Ok(Placements {
            iterator,
            graphics: self,
        })
    }
}

impl PlacementIterator {
    pub fn new() -> Result<Self> {
        let mut raw = std::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_iterator_new(std::ptr::null(), &raw mut raw)
        };
        from_result(result)?;
        Ok(Self {
            raw: NonNull::new(raw).ok_or(Error::InvalidValue)?,
            _not_send_or_sync: PhantomData,
        })
    }
}

impl Placements<'_, '_, '_> {
    #[expect(
        clippy::should_implement_trait,
        reason = "this is a lending iterator whose item borrows the iterator"
    )]
    pub fn next(&mut self) -> Option<&Self> {
        unsafe {
            ffi::ghostty_kitty_graphics_placement_next(self.iterator.raw.as_ptr()).then_some(self)
        }
    }

    /// Returns the current placement with an image tied to this iterator borrow.
    pub fn placement(&self) -> Result<Placement<'_>> {
        let mut image_id = 0_u32;
        let mut placement_id = 0_u32;
        let mut is_virtual = false;
        let mut x_offset = 0_u32;
        let mut y_offset = 0_u32;
        let mut z = 0_i32;
        let keys = [
            ffi::KittyGraphicsPlacementData::IMAGE_ID,
            ffi::KittyGraphicsPlacementData::PLACEMENT_ID,
            ffi::KittyGraphicsPlacementData::IS_VIRTUAL,
            ffi::KittyGraphicsPlacementData::X_OFFSET,
            ffi::KittyGraphicsPlacementData::Y_OFFSET,
            ffi::KittyGraphicsPlacementData::Z,
        ];
        let mut values = [
            std::ptr::from_mut(&mut image_id).cast::<c_void>(),
            std::ptr::from_mut(&mut placement_id).cast::<c_void>(),
            std::ptr::from_mut(&mut is_virtual).cast::<c_void>(),
            std::ptr::from_mut(&mut x_offset).cast::<c_void>(),
            std::ptr::from_mut(&mut y_offset).cast::<c_void>(),
            std::ptr::from_mut(&mut z).cast::<c_void>(),
        ];
        let mut written = 0;
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_get_multi(
                self.iterator.raw.as_ptr(),
                keys.len(),
                keys.as_ptr(),
                values.as_mut_ptr(),
                &raw mut written,
            )
        };
        from_result(result)?;
        if written != keys.len() {
            return Err(Error::InvalidValue);
        }

        let image =
            unsafe { ffi::ghostty_kitty_graphics_image(self.graphics.raw.as_ptr(), image_id) };
        let image = Image {
            raw: NonNull::new(image.cast_mut()).ok_or(Error::InvalidValue)?,
            _graphics: PhantomData,
        };
        let geometry = self.geometry(&image)?;

        Ok(Placement {
            image_id,
            placement_id,
            is_virtual,
            x_offset,
            y_offset,
            z,
            geometry,
            image,
        })
    }

    fn geometry(&self, image: &Image<'_>) -> Result<PlacementGeometry> {
        let mut raw = ffi::KittyGraphicsPlacementRenderInfo {
            size: size_of::<ffi::KittyGraphicsPlacementRenderInfo>(),
            ..Default::default()
        };
        let result = unsafe {
            ffi::ghostty_kitty_graphics_placement_render_info(
                self.iterator.raw.as_ptr(),
                image.raw.as_ptr(),
                self.graphics.terminal.as_raw(),
                &raw mut raw,
            )
        };
        from_result(result)?;
        Ok(PlacementGeometry {
            pixel_width: raw.pixel_width,
            pixel_height: raw.pixel_height,
            grid_columns: raw.grid_cols,
            grid_rows: raw.grid_rows,
            viewport_column: raw.viewport_col,
            viewport_row: raw.viewport_row,
            viewport_visible: raw.viewport_visible,
            source: SourceRect {
                x: raw.source_x,
                y: raw.source_y,
                width: raw.source_width,
                height: raw.source_height,
            },
        })
    }
}

impl Image<'_> {
    /// Returns decoded pixels and cache identity for this image.
    pub fn data(&self) -> Result<ImageData<'_>> {
        let mut id = 0_u32;
        let mut number = 0_u32;
        let mut width = 0_u32;
        let mut height = 0_u32;
        let mut format = ffi::KittyImageFormat::PNG;
        let mut compression = ffi::KittyImageCompression::ZLIB_DEFLATE;
        let mut data_ptr: *const u8 = std::ptr::null();
        let mut data_len = 0_usize;
        let mut generation = 0_u64;
        let keys = [
            ffi::KittyGraphicsImageData::ID,
            ffi::KittyGraphicsImageData::NUMBER,
            ffi::KittyGraphicsImageData::WIDTH,
            ffi::KittyGraphicsImageData::HEIGHT,
            ffi::KittyGraphicsImageData::FORMAT,
            ffi::KittyGraphicsImageData::COMPRESSION,
            ffi::KittyGraphicsImageData::DATA_PTR,
            ffi::KittyGraphicsImageData::DATA_LEN,
            ffi::KittyGraphicsImageData::GENERATION,
        ];
        let mut values = [
            std::ptr::from_mut(&mut id).cast::<c_void>(),
            std::ptr::from_mut(&mut number).cast::<c_void>(),
            std::ptr::from_mut(&mut width).cast::<c_void>(),
            std::ptr::from_mut(&mut height).cast::<c_void>(),
            std::ptr::from_mut(&mut format).cast::<c_void>(),
            std::ptr::from_mut(&mut compression).cast::<c_void>(),
            std::ptr::from_mut(&mut data_ptr).cast::<c_void>(),
            std::ptr::from_mut(&mut data_len).cast::<c_void>(),
            std::ptr::from_mut(&mut generation).cast::<c_void>(),
        ];
        let mut written = 0;
        let result = unsafe {
            ffi::ghostty_kitty_graphics_image_get_multi(
                self.raw.as_ptr(),
                keys.len(),
                keys.as_ptr(),
                values.as_mut_ptr(),
                &raw mut written,
            )
        };
        from_result(result)?;
        if written != keys.len()
            || compression != ffi::KittyImageCompression::NONE
            || generation == 0
        {
            return Err(Error::InvalidValue);
        }

        let format = PixelFormat::from_raw(format)?;
        let expected_len = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(format.bytes_per_pixel() as u64))
            .and_then(|length| usize::try_from(length).ok())
            .ok_or(Error::InvalidValue)?;
        if expected_len == 0 || data_len != expected_len || data_ptr.is_null() {
            return Err(Error::InvalidValue);
        }
        let pixels = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };

        Ok(ImageData {
            id,
            number,
            width,
            height,
            format,
            generation,
            pixels,
        })
    }
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
            Self::GrayAlpha => 2,
            Self::Gray => 1,
        }
    }

    fn from_raw(format: ffi::KittyImageFormat::Type) -> Result<Self> {
        match format {
            ffi::KittyImageFormat::RGB => Ok(Self::Rgb),
            ffi::KittyImageFormat::RGBA => Ok(Self::Rgba),
            ffi::KittyImageFormat::GRAY_ALPHA => Ok(Self::GrayAlpha),
            ffi::KittyImageFormat::GRAY => Ok(Self::Gray),
            _ => Err(Error::InvalidValue),
        }
    }
}

impl PlacementLayer {
    const fn as_raw(self) -> ffi::KittyPlacementLayer::Type {
        match self {
            Self::All => ffi::KittyPlacementLayer::ALL,
            Self::BelowBackground => ffi::KittyPlacementLayer::BELOW_BG,
            Self::BelowText => ffi::KittyPlacementLayer::BELOW_TEXT,
            Self::AboveText => ffi::KittyPlacementLayer::ABOVE_TEXT,
        }
    }
}

impl Drop for PlacementIterator {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_kitty_graphics_placement_iterator_free(self.raw.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::ghostty::TerminalOptions;

    const STORAGE_LIMIT: u64 = 1024 * 1024;
    const DIRECT_RGB: &[u8] = b"\x1b_Ga=T,t=d,f=24,i=1,p=1,s=1,v=2,c=10,r=1;////////\x1b\\";

    #[test]
    fn lends_direct_pixels_and_resolved_geometry() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(DIRECT_RGB);

        let graphics = terminal.graphics().unwrap();
        assert!(graphics.generation().unwrap() > 0);
        let mut iterator = PlacementIterator::new().unwrap();
        let mut placements = graphics
            .placements(&mut iterator, PlacementLayer::All)
            .unwrap();
        {
            let current = placements.next().unwrap();
            let placement = current.placement().unwrap();

            assert_eq!(placement.image_id, 1);
            assert_eq!(placement.placement_id, 1);
            assert!(!placement.is_virtual);
            assert_eq!(placement.x_offset, 0);
            assert_eq!(placement.y_offset, 0);
            assert_eq!(placement.z, 0);
            assert_eq!(
                placement.geometry,
                PlacementGeometry {
                    pixel_width: 100,
                    pixel_height: 20,
                    grid_columns: 10,
                    grid_rows: 1,
                    viewport_column: 0,
                    viewport_row: 0,
                    viewport_visible: true,
                    source: SourceRect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 2,
                    },
                }
            );

            let image = placement.image.data().unwrap();
            assert_eq!(image.id, 1);
            assert_eq!(image.width, 1);
            assert_eq!(image.height, 2);
            assert_eq!(image.format, PixelFormat::Rgb);
            assert!(image.generation > 0);
            assert_eq!(image.pixels, &[0xff; 6]);
        }
        assert!(placements.next().is_none());
    }

    #[test]
    fn filters_shared_image_placements_by_layer() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2;////////\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=1,z=5;\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=2,z=-1;\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=3,z=-1073741825;\x1b\\");

        let graphics = terminal.graphics().unwrap();
        let mut iterator = PlacementIterator::new().unwrap();
        let mut generation = None;
        for (layer, expected_z) in [
            (PlacementLayer::BelowBackground, -1_073_741_825),
            (PlacementLayer::BelowText, -1),
            (PlacementLayer::AboveText, 5),
        ] {
            let mut placements = graphics.placements(&mut iterator, layer).unwrap();
            {
                let current = placements.next().unwrap();
                let placement = current.placement().unwrap();
                assert_eq!(placement.z, expected_z);
                assert_eq!(placement.image_id, 1);
                let image_generation = placement.image.data().unwrap().generation;
                assert_eq!(
                    *generation.get_or_insert(image_generation),
                    image_generation
                );
            }
            assert!(placements.next().is_none());
        }
    }

    #[test]
    fn tracks_crop_replacement_and_deletion_generations() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(
            concat!(
                "\x1b_Ga=t,t=d,f=32,i=1,s=4,v=4;",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
                "\x1b\\",
                "\x1b_Ga=p,i=1,p=1,x=1,y=1,w=2,h=2;\x1b\\"
            )
            .as_bytes(),
        );

        let (storage_generation, image_generation) = {
            let graphics = terminal.graphics().unwrap();
            let storage_generation = graphics.generation().unwrap();
            let mut iterator = PlacementIterator::new().unwrap();
            let mut placements = graphics
                .placements(&mut iterator, PlacementLayer::All)
                .unwrap();
            let placement = placements.next().unwrap().placement().unwrap();
            assert_eq!(
                placement.geometry.source,
                SourceRect {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 2,
                }
            );
            let image_generation = placement.image.data().unwrap().generation;
            (storage_generation, image_generation)
        };

        terminal.vt_write(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2;AAAAAAAA\x1b\\");
        let replacement_generation = {
            let graphics = terminal.graphics().unwrap();
            assert!(graphics.generation().unwrap() > storage_generation);
            let mut iterator = PlacementIterator::new().unwrap();
            let mut placements = graphics
                .placements(&mut iterator, PlacementLayer::All)
                .unwrap();
            let placement = placements.next().unwrap().placement().unwrap();
            let image = placement.image.data().unwrap();
            assert_eq!(image.pixels, &[0; 6]);
            image.generation
        };
        assert!(replacement_generation > image_generation);

        terminal.vt_write(b"\x1b_Ga=d,d=A\x1b\\");
        let graphics = terminal.graphics().unwrap();
        assert!(graphics.generation().unwrap() > replacement_generation);
        let mut iterator = PlacementIterator::new().unwrap();
        let mut placements = graphics
            .placements(&mut iterator, PlacementLayer::All)
            .unwrap();
        assert!(placements.next().is_none());
    }

    #[test]
    fn rejects_file_media_while_direct_media_remains_enabled() {
        const FILE_PATH: &str = "target/kitty-medium-test.rgb";
        const FILE_PATH_BASE64: &str = "dGFyZ2V0L2tpdHR5LW1lZGl1bS10ZXN0LnJnYg==";

        fs::create_dir_all("target").unwrap();
        fs::write(FILE_PATH, [0xff; 6]).unwrap();

        let mut terminal = graphics_terminal();
        assert_direct_only(&terminal);
        terminal.vt_write(
            format!("\x1b_Ga=T,t=f,f=24,i=9,p=1,s=1,v=2;{FILE_PATH_BASE64}\x1b\\").as_bytes(),
        );

        let graphics = terminal.graphics().unwrap();
        assert_eq!(graphics.generation().unwrap(), 0);
        let mut iterator = PlacementIterator::new().unwrap();
        let mut placements = graphics
            .placements(&mut iterator, PlacementLayer::All)
            .unwrap();
        assert!(placements.next().is_none());
        fs::remove_file(FILE_PATH).unwrap();
    }

    fn graphics_terminal() -> Terminal {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 100,
        })
        .unwrap();
        terminal
            .enable_direct_graphics(STORAGE_LIMIT)
            .unwrap()
            .resize(80, 24, 10, 20)
            .unwrap();
        terminal
    }

    fn assert_direct_only(terminal: &Terminal) {
        let mut file = true;
        let result = unsafe {
            ffi::ghostty_terminal_get(
                terminal.as_raw(),
                ffi::TerminalData::KITTY_IMAGE_MEDIUM_FILE,
                std::ptr::from_mut(&mut file).cast(),
            )
        };
        from_result(result).unwrap();
        assert!(!file);

        let mut temporary_file = ffi::String::default();
        let result = unsafe {
            ffi::ghostty_terminal_get(
                terminal.as_raw(),
                ffi::TerminalData::KITTY_IMAGE_MEDIUM_TEMP_FILE,
                std::ptr::from_mut(&mut temporary_file).cast(),
            )
        };
        from_result(result).unwrap();
        assert_eq!(temporary_file.len, 0);

        let mut shared_memory = true;
        let result = unsafe {
            ffi::ghostty_terminal_get(
                terminal.as_raw(),
                ffi::TerminalData::KITTY_IMAGE_MEDIUM_SHARED_MEM,
                std::ptr::from_mut(&mut shared_memory).cast(),
            )
        };
        from_result(result).unwrap();
        assert!(!shared_memory);
    }
}
