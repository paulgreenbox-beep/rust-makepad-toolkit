//! Viewport-only PNG export using the viewer's public scene and camera data.
//!
//! This deliberately avoids capturing the whole application window, so the
//! exported image contains only the 3D model area and is independent of panel
//! layout and display scaling.

use std::fs::File;
use std::path::Path;

use viewer_core::{Vec3, Viewer};

const BACKGROUND: [u8; 4] = [18, 25, 33, 255];
const MAX_DIMENSION: usize = 4096;
const MAX_PIXELS: usize = 16 * 1024 * 1024;
pub(crate) const MAX_GPU_READBACK_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn rgba_readback_size(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("readback dimensions must be non-zero".to_owned());
    }
    if width as usize > MAX_DIMENSION || height as usize > MAX_DIMENSION {
        return Err(format!(
            "readback dimensions exceed the {MAX_DIMENSION}-pixel limit"
        ));
    }
    let byte_count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "readback size overflow".to_owned())?;
    if byte_count > MAX_GPU_READBACK_BYTES {
        return Err(format!(
            "readback exceeds the {MAX_GPU_READBACK_BYTES}-byte limit"
        ));
    }
    Ok(byte_count)
}

pub(crate) fn readback_row_layout(
    width: u32,
    height: u32,
    row_pitch: usize,
) -> Result<(usize, usize), String> {
    let _ = rgba_readback_size(width, height)?;
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| "readback row size overflow".to_owned())?;
    if row_pitch < row_bytes {
        return Err(format!(
            "mapped row pitch {row_pitch} is smaller than the {row_bytes}-byte row"
        ));
    }
    let padded_budget_bytes = row_pitch
        .checked_mul(height as usize)
        .ok_or_else(|| "mapped readback size overflow".to_owned())?;
    if padded_budget_bytes > MAX_GPU_READBACK_BYTES {
        return Err(format!(
            "mapped readback exceeds the {MAX_GPU_READBACK_BYTES}-byte limit"
        ));
    }
    let readable_extent = row_pitch
        .checked_mul((height - 1) as usize)
        .and_then(|bytes| bytes.checked_add(row_bytes))
        .ok_or_else(|| "mapped readback extent overflow".to_owned())?;
    Ok((readable_extent, padded_budget_bytes))
}

pub(crate) fn bgra_rows_to_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    row_pitch: usize,
) -> Result<Vec<u8>, String> {
    let output_size = rgba_readback_size(width, height)?;
    let (readable_extent, _) = readback_row_layout(width, height, row_pitch)?;
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| "readback row size overflow".to_owned())?;
    if data.len() < readable_extent {
        return Err(format!(
            "mapped readback buffer is too short: expected at least {readable_extent} readable bytes, got {}",
            data.len()
        ));
    }

    let mut rgba = vec![0; output_size];
    for y in 0..height as usize {
        let source_row = &data[y * row_pitch..y * row_pitch + row_bytes];
        let destination_row = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
        for (bgra, rgba) in source_row
            .chunks_exact(4)
            .zip(destination_row.chunks_exact_mut(4))
        {
            rgba.copy_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
        }
    }
    Ok(rgba)
}

pub(crate) fn write_rgba_png(
    path: &Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), String> {
    let expected_size = rgba_readback_size(width, height)?;
    if pixels.len() != expected_size {
        return Err(format!(
            "RGBA buffer length mismatch: expected {expected_size} bytes, got {}",
            pixels.len()
        ));
    }

    let file = File::create(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer.write_image_data(pixels).map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())
}

pub fn export_viewport_png(viewer: &Viewer, path: &Path) -> Result<(), String> {
    let viewport = viewer.viewport();
    let width = viewport.width.round() as usize;
    let height = viewport.height.round() as usize;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!("invalid or unsupported viewport size: {width}×{height}"));
    }
    let pixel_count = width.checked_mul(height).ok_or("viewport size overflow")?;
    if pixel_count > MAX_PIXELS {
        return Err(format!("viewport exceeds the {MAX_PIXELS}-pixel export limit"));
    }

    let mut pixels = vec![0_u8; pixel_count * 4];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&BACKGROUND);
    }
    let mut depth_buffer = vec![f64::INFINITY; pixel_count];

    for object in viewer.scene().objects().filter(|object| object.visible) {
        let mut color = object.color;
        if object.selected {
            color = [0.34, 0.68, 0.98, color[3]];
        }
        for triangle in object.mesh.triangles() {
            let world = triangle.vertices.map(|vertex| object.transform.transform_point(vertex));
            let projected = [
                viewer.camera().project_point(world[0], viewport),
                viewer.camera().project_point(world[1], viewport),
                viewer.camera().project_point(world[2], viewport),
            ];
            let [Ok(Some(a)), Ok(Some(b)), Ok(Some(c))] = projected else {
                continue;
            };
            rasterize_triangle(
                &mut pixels,
                &mut depth_buffer,
                width,
                height,
                [a.screen.x, a.screen.y, b.screen.x, b.screen.y, c.screen.x, c.screen.y],
                [a.depth, b.depth, c.depth],
                world,
                color,
            );
        }
    }

    write_rgba_png(path, width as u32, height as u32, &pixels)
}

#[allow(clippy::too_many_arguments)]
fn rasterize_triangle(
    pixels: &mut [u8],
    depth_buffer: &mut [f64],
    width: usize,
    height: usize,
    screen: [f64; 6],
    depth: [f64; 3],
    world: [Vec3; 3],
    color: [f32; 4],
) {
    if screen.iter().any(|value| !value.is_finite()) || depth.iter().any(|value| *value <= 0.0) {
        return;
    }
    let [x0, y0, x1, y1, x2, y2] = screen;
    let denominator = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2);
    if !denominator.is_finite() || denominator.abs() < 1.0e-9 {
        return;
    }
    let min_x = x0.min(x1).min(x2).floor().max(0.0) as usize;
    let max_x = x0.max(x1).max(x2).ceil().min((width - 1) as f64) as usize;
    let min_y = y0.min(y1).min(y2).floor().max(0.0) as usize;
    let max_y = y0.max(y1).max(y2).ceil().min((height - 1) as f64) as usize;
    if min_x > max_x || min_y > max_y {
        return;
    }

    let normal = (world[1] - world[0]).cross(world[2] - world[0]).normalized();
    let light = Vec3::new(-0.35, -0.45, 0.82).normalized();
    let shade = (0.48 + 0.52 * normal.dot(light).abs()).clamp(0.0, 1.0) as f32;
    let rgb = [
        (color[0] * shade * 255.0).round().clamp(0.0, 255.0) as u8,
        (color[1] * shade * 255.0).round().clamp(0.0, 255.0) as u8,
        (color[2] * shade * 255.0).round().clamp(0.0, 255.0) as u8,
    ];
    let alpha = (color[3] * 255.0).round().clamp(0.0, 255.0) as u8;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let w0 = ((y1 - y2) * (px - x2) + (x2 - x1) * (py - y2)) / denominator;
            let w1 = ((y2 - y0) * (px - x2) + (x0 - x2) * (py - y2)) / denominator;
            let w2 = 1.0 - w0 - w1;
            if w0 < -1.0e-8 || w1 < -1.0e-8 || w2 < -1.0e-8 {
                continue;
            }
            let reciprocal_depth = w0 / depth[0] + w1 / depth[1] + w2 / depth[2];
            if !reciprocal_depth.is_finite() || reciprocal_depth <= 0.0 {
                continue;
            }
            let z = reciprocal_depth.recip();
            let index = y * width + x;
            if z >= depth_buffer[index] {
                continue;
            }
            depth_buffer[index] = z;
            let offset = index * 4;
            pixels[offset..offset + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bgra_rows_to_rgba, readback_row_layout, rgba_readback_size, MAX_GPU_READBACK_BYTES,
    };

    #[test]
    fn converts_bgra_rows_skipping_padding_and_accepts_unpadded_last_row() {
        let mapped = [
            3, 2, 1, 4, 30, 20, 10, 40, 0xEE, 0xEE, 0xEE, 0xEE,
            7, 6, 5, 8, 70, 60, 50, 80,
        ];
        assert_eq!(readback_row_layout(2, 2, 12).unwrap(), (20, 24));
        let rgba = bgra_rows_to_rgba(&mapped, 2, 2, 12).unwrap();
        assert_eq!(
            rgba.as_slice(),
            &[1, 2, 3, 4, 10, 20, 30, 40, 5, 6, 7, 8, 50, 60, 70, 80]
        );
    }

    #[test]
    fn rejects_short_mapped_readable_extent_and_row_pitch() {
        assert!(bgra_rows_to_rgba(&[0; 19], 2, 2, 12).is_err());
        assert!(bgra_rows_to_rgba(&[], 2, 2, 7).is_err());
        assert!(bgra_rows_to_rgba(&[0; 15], 2, 2, 8).is_err());
    }

    #[test]
    fn validates_dimensions_and_rgba_byte_limit() {
        assert!(rgba_readback_size(0, 1).is_err());
        assert!(rgba_readback_size(4097, 1).is_err());
        assert_eq!(rgba_readback_size(4096, 4096).unwrap(), MAX_GPU_READBACK_BYTES);
    }

    #[test]
    fn rejects_mapped_pitch_overflow_and_limit() {
        assert!(bgra_rows_to_rgba(&[], 1, 2, usize::MAX).is_err());
        let too_wide_pitch = MAX_GPU_READBACK_BYTES / 2 + 1;
        assert!(bgra_rows_to_rgba(&[], 1, 2, too_wide_pitch).is_err());
    }
}
