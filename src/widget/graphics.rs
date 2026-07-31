//! Kitty graphics adaptation from Ghostty storage to reusable GPUI images.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(test)]
use gpui::InteractiveElement;
use gpui::{
    AnyElement, IntoElement, ObjectFit, ParentElement, Pixels, RenderImage, Styled, StyledImage,
    div, img, px,
};
use image::{Frame, ImageBuffer, Rgba};

use crate::ghostty::{
    Error, Result, Terminal,
    graphics::{
        ImageData, PixelFormat, PlacementGeometry, PlacementIterator, PlacementLayer, SourceRect,
    },
};

/// Ghostty stores decoded pixels inside this per-terminal limit.
pub(super) const DIRECT_GRAPHICS_STORAGE_LIMIT: u64 = 64 * 1024 * 1024;

// Expanding gray, gray-alpha, or RGB pixels to GPUI's BGRA format costs at
// most four output bytes for each byte counted by Ghostty's storage limit.
const GPU_CACHE_LIMIT: usize = DIRECT_GRAPHICS_STORAGE_LIMIT as usize * 4;
const BELOW_BACKGROUND_LIMIT: i32 = i32::MIN / 2;

pub(super) struct GraphicsRenderer {
    iterator: PlacementIterator,
    cache: TextureCache,
}

#[derive(Default)]
pub(super) struct GraphicsFrame {
    pub below_background: Vec<RenderedPlacement>,
    pub below_text: Vec<RenderedPlacement>,
    pub above_text: Vec<RenderedPlacement>,
}

pub(super) struct RenderedPlacement {
    image: Arc<RenderImage>,
    layout: PlacementLayout,
    z: i32,
    image_id: u32,
    placement_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PlacementLayout {
    destination_left: f32,
    destination_top: f32,
    destination_width: f32,
    destination_height: f32,
    image_left: f32,
    image_top: f32,
    image_width: f32,
    image_height: f32,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct TextureKey {
    image_id: u32,
    generation: u64,
}

struct CachedTexture {
    image: Arc<RenderImage>,
    byte_len: usize,
}

struct PendingPlacement {
    key: TextureKey,
    layout: PlacementLayout,
    z: i32,
    image_id: u32,
    placement_id: u32,
}

struct TextureCache {
    entries: HashMap<TextureKey, CachedTexture>,
    byte_len: usize,
    limit: usize,
}

impl GraphicsRenderer {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            iterator: PlacementIterator::new()?,
            cache: TextureCache::new(GPU_CACHE_LIMIT),
        })
    }

    /// Takes one owned frame while all Ghostty borrows remain inside this call.
    pub(super) fn frame(
        &mut self,
        terminal: &Terminal,
        cell_size: (Pixels, Pixels),
    ) -> Result<GraphicsFrame> {
        let graphics = terminal.graphics()?;
        let mut pending_textures = HashMap::new();
        let mut active_keys = HashSet::new();
        let mut pending_placements = Vec::new();

        {
            let mut placements = graphics.placements(&mut self.iterator, PlacementLayer::All)?;
            while let Some(current) = placements.next() {
                let placement = current.placement()?;
                if placement.is_virtual {
                    continue;
                }

                let image = placement.image.data()?;
                let Some(layout) = placement_layout(
                    placement.geometry,
                    placement.x_offset,
                    placement.y_offset,
                    image.width,
                    image.height,
                    cell_size,
                ) else {
                    continue;
                };
                let key = TextureKey {
                    image_id: image.id,
                    generation: image.generation,
                };
                active_keys.insert(key);
                if !self.cache.entries.contains_key(&key) && !pending_textures.contains_key(&key) {
                    pending_textures.insert(key, texture_from_image(image)?);
                }
                pending_placements.push(PendingPlacement {
                    key,
                    layout,
                    z: placement.z,
                    image_id: placement.image_id,
                    placement_id: placement.placement_id,
                });
            }
        }

        self.cache.finish_frame(&active_keys, pending_textures)?;

        let mut frame = GraphicsFrame::default();
        for placement in pending_placements {
            let image = self
                .cache
                .entries
                .get(&placement.key)
                .ok_or(Error::InvalidValue)?
                .image
                .clone();
            let rendered = RenderedPlacement {
                image,
                layout: placement.layout,
                z: placement.z,
                image_id: placement.image_id,
                placement_id: placement.placement_id,
            };
            match placement.z {
                z if z < BELOW_BACKGROUND_LIMIT => frame.below_background.push(rendered),
                z if z < 0 => frame.below_text.push(rendered),
                _ => frame.above_text.push(rendered),
            }
        }

        for layer in [
            &mut frame.below_background,
            &mut frame.below_text,
            &mut frame.above_text,
        ] {
            layer
                .sort_by_key(|placement| (placement.z, placement.image_id, placement.placement_id));
        }
        Ok(frame)
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache.entries.len()
    }
}

impl RenderedPlacement {
    pub(super) fn into_element(self) -> AnyElement {
        let layout = self.layout;
        let image = img(self.image)
            .absolute()
            .left(px(layout.image_left))
            .top(px(layout.image_top))
            .w(px(layout.image_width))
            .h(px(layout.image_height))
            .object_fit(ObjectFit::Fill);
        #[cfg(test)]
        let image = image.debug_selector(|| {
            format!(
                "kitty-graphics-image-{}-{}-{}",
                self.image_id, self.placement_id, self.z
            )
        });

        let placement = div()
            .absolute()
            .left(px(layout.destination_left))
            .top(px(layout.destination_top))
            .w(px(layout.destination_width))
            .h(px(layout.destination_height))
            .overflow_hidden();
        #[cfg(test)]
        let placement = placement.debug_selector(|| {
            format!(
                "kitty-graphics-clip-{}-{}-{}",
                self.image_id, self.placement_id, self.z
            )
        });

        placement.child(image).into_any_element()
    }

    #[cfg(test)]
    pub(super) fn z(&self) -> i32 {
        self.z
    }
}

impl TextureCache {
    fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            byte_len: 0,
            limit,
        }
    }

    fn finish_frame(
        &mut self,
        active_keys: &HashSet<TextureKey>,
        pending: HashMap<TextureKey, CachedTexture>,
    ) -> Result<()> {
        self.entries.retain(|key, texture| {
            let keep = active_keys.contains(key);
            if !keep {
                self.byte_len -= texture.byte_len;
            }
            keep
        });

        for (key, texture) in pending {
            if self.entries.contains_key(&key) {
                continue;
            }
            let Some(next_len) = self.byte_len.checked_add(texture.byte_len) else {
                return Err(Error::OutOfMemory);
            };
            if next_len > self.limit {
                return Err(Error::OutOfMemory);
            }
            self.entries.insert(key, texture);
            self.byte_len = next_len;
        }
        Ok(())
    }
}

fn texture_from_image(image: ImageData<'_>) -> Result<CachedTexture> {
    let byte_len = usize::try_from(image.width)
        .ok()
        .and_then(|width| {
            usize::try_from(image.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(Error::OutOfMemory)?;
    if byte_len > GPU_CACHE_LIMIT {
        return Err(Error::OutOfMemory);
    }

    let mut bgra = Vec::with_capacity(byte_len);
    match image.format {
        PixelFormat::Rgb => {
            for pixel in image.pixels.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], u8::MAX]);
            }
        }
        PixelFormat::Rgba => {
            for pixel in image.pixels.chunks_exact(4) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
        PixelFormat::GrayAlpha => {
            for pixel in image.pixels.chunks_exact(2) {
                bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        PixelFormat::Gray => {
            for value in image.pixels {
                bgra.extend_from_slice(&[*value, *value, *value, u8::MAX]);
            }
        }
    }
    if bgra.len() != byte_len {
        return Err(Error::InvalidValue);
    }

    let buffer = ImageBuffer::<Rgba<u8>, _>::from_raw(image.width, image.height, bgra)
        .ok_or(Error::InvalidValue)?;
    Ok(CachedTexture {
        image: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
        byte_len,
    })
}

fn placement_layout(
    geometry: PlacementGeometry,
    x_offset: u32,
    y_offset: u32,
    image_width: u32,
    image_height: u32,
    cell_size: (Pixels, Pixels),
) -> Option<PlacementLayout> {
    if !geometry.viewport_visible
        || geometry.pixel_width == 0
        || geometry.pixel_height == 0
        || geometry.grid_columns == 0
        || geometry.grid_rows == 0
        || image_width == 0
        || image_height == 0
        || geometry.source.width == 0
        || geometry.source.height == 0
        || !source_fits_image(geometry.source, image_width, image_height)
    {
        return None;
    }

    let cell_width = f32::from(cell_size.0);
    let cell_height = f32::from(cell_size.1);
    if !cell_width.is_finite()
        || !cell_height.is_finite()
        || cell_width <= 0.0
        || cell_height <= 0.0
    {
        return None;
    }

    // Terminal::resize receives integer pixel sizes. Scale Ghostty's integer
    // geometry back to GPUI's fractional cell dimensions to keep grid edges aligned.
    let terminal_cell_width = cell_width as u32;
    let terminal_cell_height = cell_height as u32;
    if terminal_cell_width == 0 || terminal_cell_height == 0 {
        return None;
    }
    let terminal_scale_x = cell_width / terminal_cell_width as f32;
    let terminal_scale_y = cell_height / terminal_cell_height as f32;
    let destination_width = geometry.pixel_width as f32 * terminal_scale_x;
    let destination_height = geometry.pixel_height as f32 * terminal_scale_y;
    let source_scale_x = destination_width / geometry.source.width as f32;
    let source_scale_y = destination_height / geometry.source.height as f32;

    Some(PlacementLayout {
        destination_left: geometry.viewport_column as f32 * cell_width
            + x_offset as f32 * terminal_scale_x,
        destination_top: geometry.viewport_row as f32 * cell_height
            + y_offset as f32 * terminal_scale_y,
        destination_width,
        destination_height,
        image_left: -(geometry.source.x as f32 * source_scale_x),
        image_top: -(geometry.source.y as f32 * source_scale_y),
        image_width: image_width as f32 * source_scale_x,
        image_height: image_height as f32 * source_scale_y,
    })
}

fn source_fits_image(source: SourceRect, image_width: u32, image_height: u32) -> bool {
    source
        .x
        .checked_add(source.width)
        .is_some_and(|right| right <= image_width)
        && source
            .y
            .checked_add(source.height)
            .is_some_and(|bottom| bottom <= image_height)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::px;

    use super::*;
    use crate::ghostty::TerminalOptions;

    #[test]
    fn shared_placements_reuse_one_gpui_image() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2;////////\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=1,z=-1;\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=2,z=-1;\x1b\\");

        let mut renderer = GraphicsRenderer::new().unwrap();
        let frame = renderer.frame(&terminal, (px(10.0), px(20.0))).unwrap();

        assert_eq!(frame.below_text.len(), 2);
        assert!(Arc::ptr_eq(
            &frame.below_text[0].image,
            &frame.below_text[1].image
        ));
        assert_eq!(renderer.cache_len(), 1);
    }

    #[test]
    fn replacement_generation_invalidates_the_cached_image() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(b"\x1b_Ga=T,t=d,f=24,i=1,p=1,s=1,v=2;////////\x1b\\");

        let mut renderer = GraphicsRenderer::new().unwrap();
        let first = renderer.frame(&terminal, (px(10.0), px(20.0))).unwrap();
        let first_image = first.above_text[0].image.clone();

        terminal.vt_write(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2;AAAAAAAA\x1b\\");
        let replacement = renderer.frame(&terminal, (px(10.0), px(20.0))).unwrap();

        assert!(!Arc::ptr_eq(&first_image, &replacement.above_text[0].image));
        assert_eq!(renderer.cache_len(), 1);
        assert_eq!(
            replacement.above_text[0].image.as_bytes(0).unwrap(),
            &[0, 0, 0, u8::MAX, 0, 0, 0, u8::MAX]
        );
    }

    #[test]
    fn crop_and_fractional_cells_resolve_to_a_clipped_full_image() {
        let layout = placement_layout(
            PlacementGeometry {
                pixel_width: 18,
                pixel_height: 38,
                grid_columns: 2,
                grid_rows: 2,
                viewport_column: -1,
                viewport_row: 3,
                viewport_visible: true,
                source: SourceRect {
                    x: 1,
                    y: 2,
                    width: 2,
                    height: 2,
                },
            },
            2,
            3,
            4,
            6,
            (px(9.6), px(19.2)),
        )
        .unwrap();

        for (actual, expected) in [
            (layout.destination_left, -7.466667),
            (layout.destination_top, 60.63158),
            (layout.destination_width, 19.2),
            (layout.destination_height, 38.4),
            (layout.image_left, -9.6),
            (layout.image_top, -38.4),
            (layout.image_width, 38.4),
            (layout.image_height, 115.2),
        ] {
            assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
        }
    }

    #[test]
    fn sorts_placements_into_ghostty_z_layers() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(b"\x1b_Ga=t,t=d,f=24,i=1,s=1,v=2;////////\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=1,z=3;\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=2,z=-1;\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=3,z=-1073741825;\x1b\\");
        terminal.vt_write(b"\x1b_Ga=p,i=1,p=4,z=0;\x1b\\");

        let mut renderer = GraphicsRenderer::new().unwrap();
        let frame = renderer.frame(&terminal, (px(10.0), px(20.0))).unwrap();

        assert_eq!(
            frame
                .below_background
                .iter()
                .map(|placement| placement.z)
                .collect::<Vec<_>>(),
            [-1_073_741_825]
        );
        assert_eq!(
            frame
                .below_text
                .iter()
                .map(|placement| placement.z)
                .collect::<Vec<_>>(),
            [-1]
        );
        assert_eq!(
            frame
                .above_text
                .iter()
                .map(|placement| placement.z)
                .collect::<Vec<_>>(),
            [0, 3]
        );
    }

    #[test]
    fn deletion_evicts_the_gpui_image() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(b"\x1b_Ga=T,t=d,f=24,i=1,p=1,s=1,v=2;////////\x1b\\");
        let mut renderer = GraphicsRenderer::new().unwrap();
        assert_eq!(
            renderer
                .frame(&terminal, (px(10.0), px(20.0)))
                .unwrap()
                .above_text
                .len(),
            1
        );
        assert_eq!(renderer.cache_len(), 1);

        terminal.vt_write(b"\x1b_Ga=d,d=A\x1b\\");
        let frame = renderer.frame(&terminal, (px(10.0), px(20.0))).unwrap();

        assert!(frame.below_background.is_empty());
        assert!(frame.below_text.is_empty());
        assert!(frame.above_text.is_empty());
        assert_eq!(renderer.cache_len(), 0);
    }

    #[test]
    fn resize_recomputes_geometry_without_reuploading_pixels() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(b"\x1b_Ga=T,t=d,f=24,i=1,p=1,s=1,v=2,c=10,r=1;////////\x1b\\");
        let mut renderer = GraphicsRenderer::new().unwrap();
        let first = renderer.frame(&terminal, (px(10.0), px(20.0))).unwrap();
        let first_image = first.above_text[0].image.clone();
        assert_eq!(first.above_text[0].layout.destination_width, 100.0);

        terminal.resize(80, 24, 8, 16).unwrap();
        let resized = renderer.frame(&terminal, (px(8.5), px(16.5))).unwrap();

        assert!(Arc::ptr_eq(&first_image, &resized.above_text[0].image));
        assert_eq!(resized.above_text[0].layout.destination_width, 85.0);
    }

    #[test]
    fn converts_source_pixels_to_gpui_bgra() {
        let texture = texture_from_image(ImageData {
            id: 1,
            number: 0,
            width: 2,
            height: 1,
            format: PixelFormat::Rgba,
            generation: 1,
            pixels: &[1, 2, 3, 4, 5, 6, 7, 8],
        })
        .unwrap();

        assert_eq!(
            texture.image.as_bytes(0).unwrap(),
            &[3, 2, 1, 4, 7, 6, 5, 8]
        );
    }

    #[test]
    fn decodes_direct_png_images_with_the_bounded_ghostty_hook() {
        let mut terminal = graphics_terminal();
        terminal.vt_write(
            concat!(
                "\x1b_Ga=T,t=d,f=100,i=1,p=1;",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk",
                "+A8AAQUBAScY42YAAAAASUVORK5CYII=",
                "\x1b\\"
            )
            .as_bytes(),
        );

        let mut renderer = GraphicsRenderer::new().unwrap();
        let frame = renderer.frame(&terminal, (px(10.0), px(20.0))).unwrap();

        assert_eq!(frame.above_text.len(), 1);
        assert_eq!(frame.above_text[0].image.as_bytes(0).unwrap().len(), 4);
    }

    fn graphics_terminal() -> Terminal {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 100,
        })
        .unwrap();
        terminal
            .enable_direct_graphics(DIRECT_GRAPHICS_STORAGE_LIMIT)
            .unwrap()
            .resize(80, 24, 10, 20)
            .unwrap();
        terminal
    }
}
