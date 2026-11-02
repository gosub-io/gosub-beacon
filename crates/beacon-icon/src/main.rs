//! `beacon-icon <size> <out.png> [fill]`: the lighthouse as a `size` square app icon, its
//! circle `fill` of the square across (0.912 unless given). An `out` ending in `.ico` gets
//! every Windows icon size up to `size` instead.
//!
//! The lighthouse lives only as `beacon-core/resources/lighthouse.svg`; the packaging scripts
//! (macOS `.icns`, the Android launcher icon, the Windows `.ico`) render what they need with
//! this at build time, so no raster copy of it is kept in the repository.

use std::process::ExitCode;

use resvg::{tiny_skia, usvg};

const SVG: &str = include_str!("../../beacon-core/resources/lighthouse.svg");

/// The circle's share of the square: the margin macOS and launchers expect around an icon.
const DEFAULT_FILL: f32 = 0.912;

/// The circle's centre and radius in the SVG's own units. The art's content group is shifted
/// by (0.1, 1.79), so this is not the middle of its 192 box.
const CENTRE: (f32, f32) = (96.1, 97.79);
const RADIUS: f32 = 96.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(size), Some(out)) = (args.first().and_then(|s| s.parse::<u32>().ok()), args.get(1)) else {
        eprintln!("usage: beacon-icon <size> <out.png|out.ico> [fill]");
        return ExitCode::FAILURE;
    };
    let fill = match args.get(2).map(|s| s.parse::<f32>()) {
        None => DEFAULT_FILL,
        Some(Ok(fill)) if fill > 0.0 && fill <= 1.0 => fill,
        Some(_) => {
            eprintln!("beacon-icon: fill must be above 0 and at most 1");
            return ExitCode::FAILURE;
        }
    };
    let written = if out.to_ascii_lowercase().ends_with(".ico") {
        ico(size, fill).and_then(|bytes| std::fs::write(out, bytes).map_err(|e| e.to_string()))
    } else {
        render(size, fill).and_then(|pixmap| pixmap.save_png(out).map_err(|e| e.to_string()))
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("beacon-icon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn render(size: u32, fill: f32) -> Result<tiny_skia::Pixmap, String> {
    let tree = usvg::Tree::from_str(SVG, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size).ok_or("size must be above zero")?;
    let scale = size as f32 * fill / (2.0 * RADIUS);
    let half = size as f32 / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(half - CENTRE.0 * scale, half - CENTRE.1 * scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

/// The sizes Windows asks an `.ico` for, from the title bar to Explorer's large icons.
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// An `.ico` holding each of [`ICO_SIZES`] up to `max` as a PNG, which Windows reads since
/// Vista: a 6-byte header, a 16-byte entry per image, then the images.
fn ico(max: u32, fill: f32) -> Result<Vec<u8>, String> {
    let images = ICO_SIZES
        .iter()
        .filter(|&&size| size <= max)
        .map(|&size| Ok((size, render(size, fill)?.encode_png().map_err(|e| e.to_string())?)))
        .collect::<Result<Vec<_>, String>>()?;
    if images.is_empty() {
        return Err("an .ico needs a size of at least 16".into());
    }
    let mut out = vec![0, 0, 1, 0];
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        // 0 means 256 in the one byte a dimension gets.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([side, side, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((png.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend(png);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ico_lists_every_size_up_to_the_largest() {
        let bytes = ico(48, DEFAULT_FILL).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 4);
        let sides: Vec<u8> = (0..4).map(|i| bytes[6 + 16 * i]).collect();
        assert_eq!(sides, [16, 24, 32, 48]);
        // The first image starts right after the directory, and is a PNG.
        let offset = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
        assert_eq!(offset, 6 + 16 * 4);
        assert_eq!(&bytes[offset..offset + 4], b"\x89PNG");
    }

    #[test]
    fn a_256_icon_is_written_as_zero() {
        let bytes = ico(256, DEFAULT_FILL).unwrap();
        assert_eq!(bytes[6 + 16 * 6], 0);
    }
}
